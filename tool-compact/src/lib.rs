//! Compact OpenAI-style tool schemas without changing their validation contract.
//!
//! The crate performs no I/O, environment access, or provider calls. `ToolDef` owns
//! the full original JSON Schema; compact signatures are descriptive only and
//! every decoded argument is checked against that original schema.
//!
//! # Compact-call grammar
//!
//! ```text
//! CALL        := OPEN TOOL_NAME WS JSON_OBJECT CLOSE
//! OPEN        := "<<call "
//! TOOL_NAME   := exact, non-whitespace tool name
//! JSON_OBJECT := valid JSON object (including nested objects and arrays)
//! CLOSE       := " >>" without the leading space, i.e. the two bytes ">>"
//! ```
//!
//! Ordinary text may precede, follow, or separate calls. JSON strings honor
//! backslash escaping, so `>>` inside a string does not close a call. Tool names
//! are exact; there is no prefix or fuzzy matching.
//!
//! # Schema support
//!
//! Objects, required/optional properties, string/integer/number/boolean types,
//! arrays, nested objects/arrays, primitive enums, descriptions, and the string
//! formats `date`, `date-time`, and `email` are compacted. The signature includes
//! an explicit `...:json` marker when additional object properties are allowed.
//! Any unknown schema keyword, non-boolean `additionalProperties`, unsupported
//! type/format, or malformed shape yields a `Bypass` decision with a deterministic
//! path and reason. Bypassed schemas are never represented as compact signatures.
//!
//! The validator implements the supported subset. Since unsupported schemas are
//! rejected before decoding, constraints cannot be silently dropped.

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use std::collections::BTreeSet;
use std::fmt;

pub const CALL_GRAMMAR: &str =
    "CALL := '<<call ' TOOL_NAME WS JSON_OBJECT '>>'; exact tool names; arguments are strict JSON objects";

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ToolDef {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    pub parameters: Value,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ToolCall {
    pub name: String,
    pub arguments: Value,
}

/// An OpenAI-compatible function call. The caller may replace the deterministic
/// local ID at the router seam if its provider requires a runtime-generated ID.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct OpenAiToolCall {
    pub id: String,
    #[serde(rename = "type")]
    pub kind: String,
    pub function: OpenAiFunction,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct OpenAiFunction {
    pub name: String,
    /// JSON-encoded argument object, matching OpenAI's function-call shape.
    pub arguments: String,
}

impl ToolCall {
    pub fn to_openai(&self, id: impl Into<String>) -> Result<OpenAiToolCall, CompactError> {
        Ok(OpenAiToolCall {
            id: id.into(),
            kind: "function".to_owned(),
            function: OpenAiFunction {
                name: self.name.clone(),
                arguments: serde_json::to_string(&self.arguments).map_err(|e| {
                    CompactError::MalformedJson {
                        reason: e.to_string(),
                    }
                })?,
            },
        })
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum CompactionDecision {
    Compact { reason: String },
    Bypass { reason: String },
}

impl CompactionDecision {
    pub fn is_compact(&self) -> bool {
        matches!(self, Self::Compact { .. })
    }

    pub fn reason(&self) -> &str {
        match self {
            Self::Compact { reason } | Self::Bypass { reason } => reason,
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SchemaComplexity {
    pub properties: usize,
    pub nested_objects: usize,
    pub arrays: usize,
    pub enums: usize,
    pub description_fields: usize,
    pub unsupported_features: usize,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CompactTool {
    pub name: String,
    pub description: Option<String>,
    pub decision: CompactionDecision,
    pub complexity: SchemaComplexity,
    /// Present only for a `Compact` decision.
    pub signature: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CompactTools {
    /// Authoritative original definitions, including bypassed tools.
    pub original_tools: Vec<ToolDef>,
    pub tools: Vec<CompactTool>,
    /// Deterministic rendering of compactable tools only.
    pub rendered: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CompactError {
    DuplicateTool { name: String },
    EmptyToolName,
    UnknownTool { name: String },
    UnsupportedSchema { name: String, reason: String },
    MalformedMarker { reason: String },
    MalformedJson { reason: String },
    InvalidArguments { tool: String, reason: String },
    IncompleteStream,
    StreamAlreadyFinished,
}

impl fmt::Display for CompactError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::DuplicateTool { name } => write!(f, "duplicate tool name: {name}"),
            Self::EmptyToolName => write!(f, "tool name must not be empty"),
            Self::UnknownTool { name } => write!(f, "unknown_tool: {name}"),
            Self::UnsupportedSchema { name, reason } => {
                write!(f, "tool {name} uses a bypassed schema: {reason}")
            }
            Self::MalformedMarker { reason } => write!(f, "malformed call marker: {reason}"),
            Self::MalformedJson { reason } => write!(f, "malformed JSON: {reason}"),
            Self::InvalidArguments { tool, reason } => {
                write!(f, "invalid arguments for {tool}: {reason}")
            }
            Self::IncompleteStream => write!(f, "incomplete compact call at end of stream"),
            Self::StreamAlreadyFinished => write!(f, "stream decoder is already finished"),
        }
    }
}

impl std::error::Error for CompactError {}

/// Analyze and compact supported schemas. Unsupported schemas are returned with
/// a deterministic `Bypass` reason; malformed tool collections return an error.
pub fn encode_tools(tools: &[ToolDef]) -> Result<CompactTools, CompactError> {
    let mut names = BTreeSet::new();
    let mut compact = Vec::with_capacity(tools.len());
    for tool in tools {
        if tool.name.trim().is_empty() {
            return Err(CompactError::EmptyToolName);
        }
        if !names.insert(tool.name.clone()) {
            return Err(CompactError::DuplicateTool {
                name: tool.name.clone(),
            });
        }
        let mut complexity = SchemaComplexity::default();
        let reason = analyze_schema(&tool.parameters, "$", true, &mut complexity);
        let decision = match reason {
            Some(reason) => {
                complexity.unsupported_features += 1;
                CompactionDecision::Bypass { reason }
            }
            None => CompactionDecision::Compact {
                reason: "supported object schema".to_owned(),
            },
        };
        let signature = if decision.is_compact() {
            Some(render_tool(tool))
        } else {
            None
        };
        compact.push(CompactTool {
            name: tool.name.clone(),
            description: tool.description.clone(),
            decision,
            complexity,
            signature,
        });
    }
    let rendered = compact
        .iter()
        .filter_map(|t| t.signature.as_deref())
        .collect::<Vec<_>>()
        .join("\n");
    Ok(CompactTools {
        original_tools: tools.to_vec(),
        tools: compact,
        rendered,
    })
}

/// Return the exact original definitions retained by [`encode_tools`].
pub fn decode_tools(compact: &CompactTools) -> Result<Vec<ToolDef>, CompactError> {
    Ok(compact.original_tools.clone())
}

/// Decode all compact calls in `text`; surrounding prose is ignored. Every call
/// is validated against its original full schema before being returned.
pub fn decode_calls(text: &str, tools: &[ToolDef]) -> Result<Vec<ToolCall>, CompactError> {
    parse_calls(text, tools, false).map(|p| p.calls)
}

/// Decode compact calls directly into a deterministic OpenAI-compatible shape.
pub fn decode_openai_calls(
    text: &str,
    tools: &[ToolDef],
) -> Result<Vec<OpenAiToolCall>, CompactError> {
    decode_calls(text, tools)?
        .iter()
        .enumerate()
        .map(|(i, call)| call.to_openai(format!("call_{}", i + 1)))
        .collect()
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StreamState {
    Text,
    OpeningMarker,
    ToolName,
    JsonArguments,
    ClosingMarker,
    Complete,
}

/// Incremental decoder. It buffers chunks so markers and JSON tokens may be split
/// at arbitrary UTF-8 character boundaries. Calls are emitted once, in order.
#[derive(Clone, Debug)]
pub struct StreamDecoder {
    tools: Vec<ToolDef>,
    buffer: String,
    emitted: usize,
    state: StreamState,
    finished: bool,
}

impl StreamDecoder {
    pub fn new(tools: &[ToolDef]) -> Self {
        Self {
            tools: tools.to_vec(),
            buffer: String::new(),
            emitted: 0,
            state: StreamState::Text,
            finished: false,
        }
    }

    /// Append a chunk and return any newly completed and validated calls.
    pub fn push_chunk(&mut self, chunk: &str) -> Result<Vec<ToolCall>, CompactError> {
        if self.finished {
            return Err(CompactError::StreamAlreadyFinished);
        }
        self.buffer.push_str(chunk);
        let parsed = parse_calls(&self.buffer, &self.tools, true)?;
        self.state = if parsed.incomplete {
            partial_state(&self.buffer)
        } else if parsed.calls.len() > self.emitted {
            StreamState::Complete
        } else {
            StreamState::Text
        };
        let fresh = parsed.calls[self.emitted..].to_vec();
        self.emitted = parsed.calls.len();
        Ok(fresh)
    }

    /// Finish the stream. An open marker or unfinished call fails closed.
    pub fn finish(&mut self) -> Result<Vec<ToolCall>, CompactError> {
        if self.finished {
            return Err(CompactError::StreamAlreadyFinished);
        }
        let parsed = parse_calls(&self.buffer, &self.tools, false)?;
        let fresh = parsed.calls[self.emitted..].to_vec();
        self.emitted = parsed.calls.len();
        self.finished = true;
        self.state = if fresh.is_empty() {
            StreamState::Text
        } else {
            StreamState::Complete
        };
        Ok(fresh)
    }

    pub fn state(&self) -> StreamState {
        self.state
    }
}

#[derive(Debug)]
struct ParseOutput {
    calls: Vec<ToolCall>,
    incomplete: bool,
}

fn parse_calls(
    text: &str,
    tools: &[ToolDef],
    allow_incomplete: bool,
) -> Result<ParseOutput, CompactError> {
    let mut calls = Vec::new();
    let mut cursor = 0usize;
    const OPEN: &str = "<<call ";
    while cursor < text.len() {
        let rest = &text[cursor..];
        let Some(rel) = rest.find("<<call") else {
            let partial_open = rest == "<"
                || ["<<", "<<c", "<<ca", "<<cal", "<<call"]
                    .iter()
                    .any(|suffix| rest.ends_with(suffix));
            if partial_open {
                return if allow_incomplete {
                    Ok(ParseOutput {
                        calls,
                        incomplete: true,
                    })
                } else {
                    Err(CompactError::IncompleteStream)
                };
            }
            if rest.contains(">>") {
                return Err(CompactError::MalformedMarker {
                    reason: "closing marker without an open call".to_owned(),
                });
            }
            break;
        };
        let start = cursor + rel;
        if text[start..].len() < OPEN.len() && OPEN.starts_with(&text[start..]) {
            return if allow_incomplete {
                Ok(ParseOutput {
                    calls,
                    incomplete: true,
                })
            } else {
                Err(CompactError::IncompleteStream)
            };
        }
        if !text[start..].starts_with(OPEN) {
            return Err(CompactError::MalformedMarker {
                reason: "expected exact opener `<<call `".to_owned(),
            });
        }
        let name_start = start + OPEN.len();
        let Some(name_rel_end) = text[name_start..].find(char::is_whitespace) else {
            let name_tail = &text[name_start..];
            if name_tail.contains('{') || name_tail.contains('}') || name_tail.contains(">>") {
                return Err(CompactError::MalformedMarker {
                    reason: "expected whitespace between tool name and JSON object".to_owned(),
                });
            }
            return if allow_incomplete {
                Ok(ParseOutput {
                    calls,
                    incomplete: true,
                })
            } else {
                Err(CompactError::IncompleteStream)
            };
        };
        let name_end = name_start + name_rel_end;
        let name = &text[name_start..name_end];
        if name.is_empty() {
            return Err(CompactError::MalformedMarker {
                reason: "tool name is empty".to_owned(),
            });
        }
        let mut json_start = name_end;
        while let Some(ch) = text[json_start..].chars().next() {
            if !ch.is_whitespace() {
                break;
            }
            json_start += ch.len_utf8();
        }
        if json_start == text.len() {
            return if allow_incomplete {
                Ok(ParseOutput {
                    calls,
                    incomplete: true,
                })
            } else {
                Err(CompactError::IncompleteStream)
            };
        }
        if !text[json_start..].starts_with('{') {
            return Err(CompactError::MalformedMarker {
                reason: "arguments must be a JSON object".to_owned(),
            });
        }
        let json_end = match scan_json_object(text, json_start)? {
            Some(end) => end,
            None if allow_incomplete => {
                return Ok(ParseOutput {
                    calls,
                    incomplete: true,
                })
            }
            None => return Err(CompactError::IncompleteStream),
        };
        let mut close_start = json_end;
        while let Some(ch) = text[close_start..].chars().next() {
            if !ch.is_whitespace() {
                break;
            }
            close_start += ch.len_utf8();
        }
        if close_start == text.len()
            || (text.as_bytes().get(close_start) == Some(&b'>') && close_start + 1 == text.len())
        {
            return if allow_incomplete {
                Ok(ParseOutput {
                    calls,
                    incomplete: true,
                })
            } else {
                Err(CompactError::IncompleteStream)
            };
        }
        if !text[close_start..].starts_with(">>") {
            return Err(CompactError::MalformedMarker {
                reason: "expected `>>` immediately after the JSON object".to_owned(),
            });
        }
        let tool = find_tool(tools, name)?;
        let schema_reason = schema_bypass_reason(&tool.parameters);
        if let Some(reason) = schema_reason {
            return Err(CompactError::UnsupportedSchema {
                name: tool.name.clone(),
                reason,
            });
        }
        let arguments: Value = serde_json::from_str(&text[json_start..json_end]).map_err(|e| {
            CompactError::MalformedJson {
                reason: e.to_string(),
            }
        })?;
        validate_value(&arguments, &tool.parameters, "arguments").map_err(|reason| {
            CompactError::InvalidArguments {
                tool: tool.name.clone(),
                reason,
            }
        })?;
        calls.push(ToolCall {
            name: name.to_owned(),
            arguments,
        });
        cursor = close_start + 2;
    }
    Ok(ParseOutput {
        calls,
        incomplete: false,
    })
}

/// Return the exclusive end byte of a complete balanced root JSON object.
/// Strings and escaped quotes are tracked, so marker-like text inside a string
/// never terminates the object. `None` means the object is incomplete.
fn scan_json_object(text: &str, start: usize) -> Result<Option<usize>, CompactError> {
    let bytes = text.as_bytes();
    let mut stack = Vec::new();
    let mut in_string = false;
    let mut escaped = false;
    let mut i = start;
    while i < bytes.len() {
        let byte = bytes[i];
        if in_string {
            if escaped {
                escaped = false;
            } else if byte == b'\\' {
                escaped = true;
            } else if byte == b'"' {
                in_string = false;
            }
            i += 1;
            continue;
        }
        match byte {
            b'"' => in_string = true,
            b'{' => stack.push(b'}'),
            b'[' => stack.push(b']'),
            b'}' | b']' => {
                if stack.pop() != Some(byte) {
                    return Err(CompactError::MalformedJson {
                        reason: "mismatched object or array delimiter".to_owned(),
                    });
                }
                if stack.is_empty() {
                    return Ok(Some(i + 1));
                }
            }
            _ => {}
        }
        i += 1;
    }
    Ok(None)
}

fn partial_state(text: &str) -> StreamState {
    let Some(start) = text.rfind("<<call") else {
        let tail = text.rfind("<<").map(|index| &text[index..]).unwrap_or(text);
        return if text.ends_with('<') || ["<<", "<<c", "<<ca", "<<cal", "<<call"].contains(&tail) {
            StreamState::OpeningMarker
        } else {
            StreamState::Text
        };
    };
    let tail = &text[start..];
    if tail.len() < "<<call ".len() {
        return StreamState::OpeningMarker;
    }
    let after_open = &tail["<<call ".len()..];
    if !after_open.chars().any(char::is_whitespace) {
        return StreamState::ToolName;
    }
    if after_open.contains('{') {
        let open = after_open.find('{').unwrap_or(0);
        if scan_json_object(after_open, open).ok().flatten().is_some() {
            StreamState::ClosingMarker
        } else {
            StreamState::JsonArguments
        }
    } else {
        StreamState::ToolName
    }
}

fn find_tool<'a>(tools: &'a [ToolDef], name: &str) -> Result<&'a ToolDef, CompactError> {
    let mut found = None;
    for tool in tools {
        if tool.name == name {
            if found.is_some() {
                return Err(CompactError::DuplicateTool {
                    name: name.to_owned(),
                });
            }
            found = Some(tool);
        }
    }
    found.ok_or_else(|| CompactError::UnknownTool {
        name: name.to_owned(),
    })
}

fn analyze_schema(
    schema: &Value,
    path: &str,
    root: bool,
    meta: &mut SchemaComplexity,
) -> Option<String> {
    let Some(obj) = schema.as_object() else {
        return Some(format!("schema at {path} must be an object"));
    };
    const ALLOWED: &[&str] = &[
        "type",
        "properties",
        "required",
        "items",
        "enum",
        "format",
        "description",
        "additionalProperties",
    ];
    for key in obj.keys() {
        if !ALLOWED.contains(&key.as_str()) {
            return Some(format!("unsupported keyword `{key}` at {path}"));
        }
    }
    if obj.get("description").is_some() {
        if !obj.get("description").is_some_and(Value::is_string) {
            return Some(format!("description at {path} must be a string"));
        }
        meta.description_fields += 1;
    }
    let Some(kind) = obj.get("type").and_then(Value::as_str) else {
        return Some(format!("type is missing or not a string at {path}"));
    };
    if root && kind != "object" {
        return Some(format!("root parameters must have type `object` at {path}"));
    }
    match kind {
        "object" => {
            if !root {
                meta.nested_objects += 1;
            }
            if let Some(additional) = obj.get("additionalProperties") {
                if !additional.is_boolean() {
                    return Some(format!(
                        "schema-valued `additionalProperties` is unsupported at {path}"
                    ));
                }
            }
            let properties = match obj.get("properties") {
                None => Map::new(),
                Some(Value::Object(properties)) => properties.clone(),
                Some(_) => return Some(format!("`properties` must be an object at {path}")),
            };
            meta.properties += properties.len();
            let required: BTreeSet<&str> = match obj.get("required") {
                None => BTreeSet::new(),
                Some(Value::Array(values)) => {
                    let mut names = BTreeSet::new();
                    for value in values {
                        let Some(name) = value.as_str() else {
                            return Some(format!("`required` entries must be strings at {path}"));
                        };
                        if !properties.contains_key(name) {
                            return Some(format!(
                                "required field `{name}` is not declared at {path}"
                            ));
                        }
                        names.insert(name);
                    }
                    names
                }
                Some(_) => return Some(format!("`required` must be an array at {path}")),
            };
            let _ = required;
            for (name, property) in &properties {
                if let Some(reason) =
                    analyze_schema(property, &format!("{path}.properties.{name}"), false, meta)
                {
                    return Some(reason);
                }
            }
        }
        "array" => {
            meta.arrays += 1;
            let Some(items) = obj.get("items") else {
                return Some(format!("array items are missing at {path}"));
            };
            if let Some(reason) = analyze_schema(items, &format!("{path}.items"), false, meta) {
                return Some(reason);
            }
        }
        "string" | "integer" | "number" | "boolean" => {
            if obj.contains_key("properties")
                || obj.contains_key("required")
                || obj.contains_key("items")
                || obj.contains_key("additionalProperties")
            {
                return Some(format!(
                    "object/array keywords do not apply to `{kind}` at {path}"
                ));
            }
        }
        other => return Some(format!("unsupported type `{other}` at {path}")),
    }
    if let Some(format) = obj.get("format") {
        let Some(format) = format.as_str() else {
            return Some(format!("format at {path} must be a string"));
        };
        if kind != "string" {
            return Some(format!("format is supported only for strings at {path}"));
        }
        if !["date", "date-time", "email"].contains(&format) {
            return Some(format!("unsupported format `{format}` at {path}"));
        }
    }
    if let Some(values) = obj.get("enum") {
        let Some(values) = values.as_array() else {
            return Some(format!("enum at {path} must be an array"));
        };
        if values
            .iter()
            .any(|v| !(v.is_string() || v.is_number() || v.is_boolean() || v.is_null()))
        {
            return Some(format!(
                "only primitive enum values are supported at {path}"
            ));
        }
        meta.enums += 1;
    }
    None
}

fn schema_bypass_reason(schema: &Value) -> Option<String> {
    analyze_schema(schema, "$", true, &mut SchemaComplexity::default())
}

fn render_tool(tool: &ToolDef) -> String {
    let fields = render_object_fields(&tool.parameters);
    let mut line = format!("{}({fields})", tool.name);
    let tool_description = tool.description.as_deref().filter(|s| !s.trim().is_empty());
    let schema_description = tool
        .parameters
        .get("description")
        .and_then(Value::as_str)
        .filter(|s| !s.trim().is_empty());
    let description = match (tool_description, schema_description) {
        (Some(tool), Some(schema)) if tool.trim() != schema.trim() => {
            Some(format!("{} — {}", tool.trim(), schema.trim()))
        }
        (Some(tool), _) => Some(tool.trim().to_owned()),
        (None, Some(schema)) => Some(schema.trim().to_owned()),
        (None, None) => None,
    };
    if let Some(description) = description {
        line.push_str(" - ");
        line.push_str(&description);
    }
    line
}

fn render_object_fields(schema: &Value) -> String {
    let Some(obj) = schema.as_object() else {
        return String::new();
    };
    let properties = obj.get("properties").and_then(Value::as_object);
    let required: BTreeSet<&str> = obj
        .get("required")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .collect();
    let mut fields = properties
        .map(|p| {
            p.iter()
                .map(|(name, schema)| {
                    let optional = if required.contains(name.as_str()) {
                        ""
                    } else {
                        "?"
                    };
                    format!("{name}{optional}:{}", render_type(schema))
                })
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    if obj
        .get("additionalProperties")
        .and_then(Value::as_bool)
        .unwrap_or(true)
    {
        fields.push("...:json".to_owned());
    }
    fields.join(", ")
}

fn render_type(schema: &Value) -> String {
    let Some(obj) = schema.as_object() else {
        return "json".to_owned();
    };
    let base = if let Some(values) = obj.get("enum").and_then(Value::as_array) {
        values
            .iter()
            .map(|v| serde_json::to_string(v).unwrap_or_else(|_| "null".to_owned()))
            .collect::<Vec<_>>()
            .join("|")
    } else {
        match obj.get("type").and_then(Value::as_str).unwrap_or("json") {
            "string" => match obj.get("format").and_then(Value::as_str) {
                Some("date") => "date".to_owned(),
                Some("date-time") => "datetime".to_owned(),
                Some("email") => "email".to_owned(),
                _ => "str".to_owned(),
            },
            "integer" => "int".to_owned(),
            "number" => "float".to_owned(),
            "boolean" => "bool".to_owned(),
            "array" => format!(
                "[{}]",
                obj.get("items")
                    .map(render_type)
                    .unwrap_or_else(|| "json".to_owned())
            ),
            "object" => format!("{{{}}}", render_object_fields(schema)),
            other => other.to_owned(),
        }
    };
    if let Some(description) = obj
        .get("description")
        .and_then(Value::as_str)
        .filter(|s| !s.trim().is_empty())
    {
        format!("{base} — {}", description.trim())
    } else {
        base
    }
}

fn validate_value(value: &Value, schema: &Value, path: &str) -> Result<(), String> {
    let obj = schema
        .as_object()
        .ok_or_else(|| format!("{path}: schema must be an object"))?;
    if let Some(enumeration) = obj.get("enum").and_then(Value::as_array) {
        if !enumeration.iter().any(|v| v == value) {
            return Err(format!(
                "{path}: value is not one of {}",
                serde_json::to_string(enumeration).unwrap_or_default()
            ));
        }
    }
    let kind = obj
        .get("type")
        .and_then(Value::as_str)
        .ok_or_else(|| format!("{path}: schema type is missing"))?;
    match kind {
        "object" => {
            let Some(values) = value.as_object() else {
                return Err(format!("{path}: expected object"));
            };
            let props = obj.get("properties").and_then(Value::as_object);
            if let Some(required) = obj.get("required").and_then(Value::as_array) {
                for field in required.iter().filter_map(Value::as_str) {
                    if !values.contains_key(field) {
                        return Err(format!("{path}.{field}: required field is missing"));
                    }
                }
            }
            for (name, val) in values {
                match props.and_then(|p| p.get(name)) {
                    Some(child) => validate_value(val, child, &format!("{path}.{name}"))?,
                    None if obj.get("additionalProperties") == Some(&Value::Bool(false)) => {
                        return Err(format!("{path}.{name}: unknown argument is not allowed"));
                    }
                    None => {}
                }
            }
        }
        "array" => {
            let Some(values) = value.as_array() else {
                return Err(format!("{path}: expected array"));
            };
            let items = obj
                .get("items")
                .ok_or_else(|| format!("{path}: array items schema is missing"))?;
            for (index, item) in values.iter().enumerate() {
                validate_value(item, items, &format!("{path}[{index}]"))?;
            }
        }
        "string" => {
            let Some(text) = value.as_str() else {
                return Err(format!("{path}: expected string"));
            };
            match obj.get("format").and_then(Value::as_str) {
                Some("email") if !valid_email(text) => {
                    return Err(format!("{path}: invalid email format"))
                }
                Some("date") if !valid_date(text) => {
                    return Err(format!("{path}: invalid date format"))
                }
                Some("date-time") if !valid_datetime(text) => {
                    return Err(format!("{path}: invalid date-time format"))
                }
                _ => {}
            }
        }
        "integer" => {
            let integral_float = value
                .as_f64()
                .is_some_and(|number| number.is_finite() && number.fract() == 0.0);
            if value.as_i64().is_none() && value.as_u64().is_none() && !integral_float {
                return Err(format!("{path}: expected integer"));
            }
        }
        "number" => {
            if !value.is_number() {
                return Err(format!("{path}: expected number"));
            }
        }
        "boolean" => {
            if !value.is_boolean() {
                return Err(format!("{path}: expected boolean"));
            }
        }
        other => return Err(format!("{path}: unsupported schema type `{other}`")),
    }
    Ok(())
}

fn valid_email(text: &str) -> bool {
    let mut pieces = text.split('@');
    let Some(local) = pieces.next() else {
        return false;
    };
    let Some(domain) = pieces.next() else {
        return false;
    };
    !local.is_empty()
        && !domain.is_empty()
        && !domain.starts_with('.')
        && !domain.ends_with('.')
        && domain.contains('.')
        && !text.chars().any(char::is_whitespace)
        && pieces.next().is_none()
}

fn valid_date(text: &str) -> bool {
    let bytes = text.as_bytes();
    if bytes.len() != 10 || bytes[4] != b'-' || bytes[7] != b'-' {
        return false;
    }
    if !bytes
        .iter()
        .enumerate()
        .all(|(i, b)| matches!(i, 4 | 7) || b.is_ascii_digit())
    {
        return false;
    }
    let (Ok(year), Ok(month), Ok(day)) = (
        text[0..4].parse::<i32>(),
        text[5..7].parse::<u32>(),
        text[8..10].parse::<u32>(),
    ) else {
        return false;
    };
    let leap = year % 4 == 0 && (year % 100 != 0 || year % 400 == 0);
    let days = match month {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 if leap => 29,
        2 => 28,
        _ => return false,
    };
    day > 0 && day <= days
}

fn valid_datetime(text: &str) -> bool {
    // Strict ISO-8601 date-time subset: date, HH:MM:SS, optional fraction, and
    // either Z or a numeric timezone offset are required.
    if text.len() < 20
        || !text.get(0..10).is_some_and(valid_date)
        || !matches!(text.as_bytes()[10], b'T' | b't')
    {
        return false;
    }
    let bytes = text.as_bytes();
    if bytes.get(13) != Some(&b':') || bytes.get(16) != Some(&b':') {
        return false;
    }
    let (Ok(hour), Ok(minute), Ok(second)) = (
        text.get(11..13).unwrap_or("").parse::<u32>(),
        text.get(14..16).unwrap_or("").parse::<u32>(),
        text.get(17..19).unwrap_or("").parse::<u32>(),
    ) else {
        return false;
    };
    if hour > 23 || minute > 59 || second > 60 {
        return false;
    }
    let Some(mut rest) = text.get(19..) else {
        return false;
    };
    if rest.starts_with('.') {
        let fraction = &rest[1..];
        let digits = fraction.bytes().take_while(u8::is_ascii_digit).count();
        if digits == 0 {
            return false;
        }
        rest = &fraction[digits..];
    }
    if rest == "Z" || rest == "z" {
        return true;
    }
    if rest.len() != 6 || !matches!(rest.as_bytes()[0], b'+' | b'-') || rest.as_bytes()[3] != b':' {
        return false;
    }
    if !rest.as_bytes()[1..3].iter().all(u8::is_ascii_digit)
        || !rest.as_bytes()[4..6].iter().all(u8::is_ascii_digit)
    {
        return false;
    }
    let (Ok(oh), Ok(om)) = (rest[1..3].parse::<u32>(), rest[4..6].parse::<u32>()) else {
        return false;
    };
    oh <= 23 && om <= 59
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn tools() -> Vec<ToolDef> {
        vec![ToolDef {
            name: "create_calendar_event".into(),
            description: Some("Create an event in the user's calendar.".into()),
            parameters: json!({
                "type":"object",
                "properties":{
                    "title":{"type":"string","description":"Event title"},
                    "start":{"type":"string","format":"date-time"},
                    "duration_min":{"type":"integer"},
                    "attendees":{"type":"array","items":{"type":"string","format":"email"}},
                    "visibility":{"type":"string","enum":["public","private"]},
                    "metadata":{"type":"object","properties":{"source":{"type":"string"}},"required":["source"],"additionalProperties":false}
                },
                "required":["title","start"],
                "additionalProperties":false
            }),
        }]
    }

    #[test]
    fn compact_encoding_preserves_required_optional_and_descriptions() {
        let encoded = encode_tools(&tools()).unwrap();
        assert!(encoded.rendered.contains("title:str — Event title"));
        assert!(encoded.rendered.contains("start:datetime"));
        assert!(encoded.rendered.contains("duration_min?:int"));
        assert!(encoded
            .rendered
            .contains("visibility?:\"public\"|\"private\""));
        assert!(encoded.rendered.contains("metadata?:{source:str}"));
    }

    #[test]
    fn preserves_root_and_nested_schema_descriptions() {
        let mut tool = tools().remove(0);
        tool.parameters["description"] = json!("The event payload.");
        tool.parameters["properties"]["metadata"]["description"] = json!("Audit details.");
        let signature = encode_tools(&[tool]).unwrap().rendered;
        assert!(signature.contains("metadata?:{source:str} — Audit details."));
        assert!(signature.ends_with("Create an event in the user's calendar. — The event payload."));
    }

    #[test]
    fn schema_round_trip_is_exact_and_order_independent_comparison_is_possible() {
        let original = tools();
        let compact = encode_tools(&original).unwrap();
        let recovered = decode_tools(&compact).unwrap();
        assert_eq!(recovered, original);
        assert_eq!(
            compact.tools[0].decision,
            CompactionDecision::Compact {
                reason: "supported object schema".into()
            }
        );
    }

    #[test]
    fn unsupported_constraints_bypass_with_exact_reason() {
        let mut input = tools();
        input[0].parameters["properties"]["title"]["minLength"] = json!(2);
        let result = encode_tools(&input).unwrap();
        assert_eq!(
            result.tools[0].decision,
            CompactionDecision::Bypass {
                reason: "unsupported keyword `minLength` at $.properties.title".into()
            }
        );
        assert!(result.tools[0].signature.is_none());
    }

    #[test]
    fn decodes_multiple_calls_with_text_and_openai_conversion() {
        let input = concat!(
            "Starting. ",
            "<<call create_calendar_event {\"title\":\"Plan\",\"start\":\"2026-10-05T15:00:00+05:30\"}>>",
            " ordinary text ",
            "<<call create_calendar_event {\"title\":\"Review\",\"start\":\"2026-10-06T15:00:00Z\"}>>",
            " Done."
        );
        let calls = decode_calls(input, &tools()).unwrap();
        assert_eq!(calls.len(), 2);
        assert_eq!(calls[0].arguments["title"], "Plan");
        let openai = decode_openai_calls(input, &tools()).unwrap();
        assert_eq!(openai[0].kind, "function");
        assert_eq!(openai[0].function.name, "create_calendar_event");
        assert_eq!(openai[0].id, "call_1");
    }

    #[test]
    fn strings_with_markers_escapes_and_unicode_survive() {
        let input = "<<call create_calendar_event {\"title\":\"quote \\\" and >> and \\\\ path ☀\",\"start\":\"2026-10-05T15:00:00Z\"}>>";
        let decoded = decode_calls(input, &tools()).unwrap();
        assert_eq!(
            decoded[0].arguments["title"],
            "quote \" and >> and \\ path ☀"
        );
    }

    #[test]
    fn rejects_missing_wrong_enum_wrong_type_unknown_extra_unknown_tool_and_json() {
        let cases = [
            ("<<call create_calendar_event {\"start\":\"2026-10-05T15:00:00Z\"}>>", "required field"),
            ("<<call create_calendar_event {\"title\":2,\"start\":\"2026-10-05T15:00:00Z\"}>>", "expected string"),
            ("<<call create_calendar_event {\"title\":\"x\",\"start\":\"2026-10-05T15:00:00Z\",\"visibility\":\"secret\"}>>", "not one of"),
            ("<<call create_calendar_event {\"title\":\"x\",\"start\":\"2026-10-05T15:00:00Z\",\"duration_min\":\"3\"}>>", "expected integer"),
            ("<<call create_calendar_event {\"title\":\"x\",\"start\":\"2026-10-05T15:00:00Z\",\"extra\":true}>>", "unknown argument"),
            ("<<call create_calendar_event {\"title\":\"x\",\"start\":\"2026-10-05T15:00:00Z\"}", "incomplete"),
            ("<<call missing_tool {}>>", "unknown_tool"),
            ("<<call create_calendar_event {\"title\":}>>", "malformed JSON"),
        ];
        for (input, expected) in cases {
            let error = decode_calls(input, &tools()).unwrap_err().to_string();
            assert!(error.contains(expected), "{input}: {error}");
        }
    }

    #[test]
    fn malformed_markers_and_stray_closes_fail_closed() {
        assert!(matches!(
            decode_calls("<<call", &tools()),
            Err(CompactError::IncompleteStream)
        ));
        assert!(matches!(
            decode_calls("<<callable x>>", &tools()),
            Err(CompactError::MalformedMarker { .. })
        ));
        assert!(matches!(
            decode_calls("<<call create_calendar_event{}>>", &tools()),
            Err(CompactError::MalformedMarker { .. })
        ));
        assert!(matches!(
            decode_calls("text >>", &tools()),
            Err(CompactError::MalformedMarker { .. })
        ));
    }

    #[test]
    fn rejects_nested_and_array_item_violations_and_duplicate_names() {
        let defs = tools();
        let nested = "<<call create_calendar_event {\"title\":\"x\",\"start\":\"2026-10-05T15:00:00Z\",\"metadata\":{\"source\":3}}>>";
        assert!(decode_calls(nested, &defs)
            .unwrap_err()
            .to_string()
            .contains("arguments.metadata.source: expected string"));
        let array = "<<call create_calendar_event {\"title\":\"x\",\"start\":\"2026-10-05T15:00:00Z\",\"attendees\":[\"not-an-email\"]}>>";
        assert!(decode_calls(array, &defs)
            .unwrap_err()
            .to_string()
            .contains("arguments.attendees[0]: invalid email"));
        let duplicate = vec![defs[0].clone(), defs[0].clone()];
        assert!(matches!(
            encode_tools(&duplicate),
            Err(CompactError::DuplicateTool { .. })
        ));
    }

    #[test]
    fn stream_decoder_handles_every_two_chunk_boundary() {
        let input = "Before <<call create_calendar_event {\"title\":\"Split ☀ >> string\",\"start\":\"2026-10-05T15:00:00Z\"}>> after";
        for split in input
            .char_indices()
            .map(|(i, _)| i)
            .chain(std::iter::once(input.len()))
        {
            let mut decoder = StreamDecoder::new(&tools());
            let mut calls = decoder.push_chunk(&input[..split]).unwrap();
            calls.extend(decoder.push_chunk(&input[split..]).unwrap());
            calls.extend(decoder.finish().unwrap());
            assert_eq!(calls.len(), 1, "split={split}");
            assert_eq!(calls[0].arguments["title"], "Split ☀ >> string");
        }
    }

    #[test]
    fn stream_emits_each_call_once_and_rejects_incomplete_finish() {
        let mut decoder = StreamDecoder::new(&tools());
        let call =
            "<<call create_calendar_event {\"title\":\"A\",\"start\":\"2026-10-05T15:00:00Z\"}>>";
        assert_eq!(decoder.push_chunk(call).unwrap().len(), 1);
        assert!(decoder.push_chunk(" trailing ").unwrap().is_empty());
        assert!(decoder.finish().unwrap().is_empty());
        assert!(matches!(
            decoder.push_chunk("x"),
            Err(CompactError::StreamAlreadyFinished)
        ));

        let mut incomplete = StreamDecoder::new(&tools());
        incomplete.push_chunk("<<ca").unwrap();
        assert!(matches!(
            incomplete.finish(),
            Err(CompactError::IncompleteStream)
        ));
    }

    #[test]
    fn property_round_trips_supported_schema_families() {
        // Deterministic generated family sweep: each generated tool uses varied
        // names, descriptions, requiredness, enums, nested objects, and arrays.
        for n in 0..64 {
            let field = format!("field_{n}");
            let schema = json!({
                "type":"object",
                "properties":{
                    (field.clone()):{"type":"string","description":format!("label {n}")},
                    "count":{"type":"integer"},
                    "flags":{"type":"array","items":{"type":"boolean"}},
                    "nested":{"type":"object","properties":{"mode":{"type":"string","enum":["a","b"]}},"required":["mode"],"additionalProperties":false}
                },
                "required":[field.clone()],
                "additionalProperties":false
            });
            let def = ToolDef {
                name: format!("generated_{n}"),
                description: Some(format!("Generated {n}")),
                parameters: schema,
            };
            let encoded = encode_tools(std::slice::from_ref(&def)).unwrap();
            assert!(encoded.tools[0].decision.is_compact());
            assert_eq!(decode_tools(&encoded).unwrap(), vec![def.clone()]);
            let arguments = json!({(field.clone()):format!("value {n}"),"count":n,"flags":[true,false],"nested":{"mode":"a"}});
            let rendered = format!(
                "<<call generated_{n} {}>>",
                serde_json::to_string(&arguments).unwrap()
            );
            let decoded = decode_calls(&rendered, std::slice::from_ref(&def)).unwrap();
            assert_eq!(decoded[0].arguments, arguments);
            let mut invalid_arguments = arguments;
            invalid_arguments["count"] = json!("not-an-integer");
            let invalid = format!(
                "<<call generated_{n} {}>>",
                serde_json::to_string(&invalid_arguments).unwrap()
            );
            assert!(decode_calls(&invalid, &[def])
                .unwrap_err()
                .to_string()
                .contains("expected integer"));
        }
    }

    #[test]
    fn allowed_extra_arguments_are_explicit_and_remain_allowed() {
        let permissive = vec![ToolDef {
            name: "permissive".into(),
            description: None,
            parameters: json!({"type":"object","properties":{"x":{"type":"string"}}}),
        }];
        let rendered = encode_tools(&permissive).unwrap().rendered;
        assert!(rendered.contains("...:json"));
        assert!(decode_calls(
            "<<call permissive {\"x\":\"ok\",\"extra\":3}>>",
            &permissive
        )
        .is_ok());
    }

    #[test]
    fn invalid_unicode_format_values_fail_without_panicking() {
        let defs = vec![ToolDef {
            name: "date_tool".into(),
            description: None,
            parameters: json!({"type":"object","properties":{"when":{"type":"string","format":"date-time"}},"required":["when"],"additionalProperties":false}),
        }];
        for value in [
            "☀☀☀☀-01-01T00:00:00Z",
            "2025-13-40T00:00:00Z",
            "2025-01-01T00:00:00☀",
        ] {
            let input = format!(
                "<<call date_tool {}>>",
                serde_json::to_string(&json!({"when":value})).unwrap()
            );
            assert!(decode_calls(&input, &defs).is_err());
        }
        assert!(decode_calls("ordinary text ends with <", &defs).is_ok());
    }
}
