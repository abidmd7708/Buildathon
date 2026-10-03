//! Deterministic offline CompactCall evaluation adapter.
//!
//! Required environment variables: `EVAL_SET` and `OUT`. No API key or network
//! connection is used. Supported dataset roots are an array of cases or an
//! object containing `cases`/`normal_cases` and optional `decoder_cases` arrays.

use nasiko_tool_compact::{
    decode_calls, encode_tools, CompactTools, StreamDecoder, ToolCall, ToolDef, CALL_GRAMMAR,
};
use serde_json::{json, Map, Value};
use std::env;
use std::fs;
use std::process::ExitCode;

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(message) => {
            eprintln!("compact_tools_eval: {message}");
            ExitCode::FAILURE
        }
    }
}

fn run() -> Result<(), String> {
    let input_path =
        env::var("EVAL_SET").map_err(|_| "EVAL_SET must name the evaluation dataset")?;
    let output_path = env::var("OUT").map_err(|_| "OUT must name the output JSONL file")?;
    let source = fs::read_to_string(&input_path)
        .map_err(|e| format!("could not read EVAL_SET `{input_path}`: {e}"))?;
    // PowerShell and some editors write a UTF-8 BOM; accept it as JSON input.
    let source = source.strip_prefix('\u{feff}').unwrap_or(&source);
    let dataset: Value =
        serde_json::from_str(source).map_err(|e| format!("EVAL_SET is not valid JSON: {e}"))?;
    let records = evaluate_dataset(&dataset)?;
    let mut jsonl = records
        .iter()
        .map(serde_json::to_string)
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| format!("could not serialize JSONL output: {e}"))?
        .join("\n");
    if !jsonl.is_empty() {
        jsonl.push('\n');
    }
    fs::write(&output_path, jsonl)
        .map_err(|e| format!("could not write OUT `{output_path}`: {e}"))?;
    println!(
        "wrote {} deterministic evaluation records to {}",
        records.len(),
        output_path
    );
    Ok(())
}

fn evaluate_dataset(dataset: &Value) -> Result<Vec<Value>, String> {
    let (global_tools, normal_cases, decoder_cases) = match dataset {
        Value::Array(rows) => (Vec::new(), rows.as_slice(), &[][..]),
        Value::Object(root) => {
            let global = root
                .get("tools")
                .or_else(|| root.get("tool_definitions"))
                .map(parse_tools)
                .transpose()?
                .unwrap_or_default();
            let normal = root
                .get("normal_cases")
                .or_else(|| root.get("cases"))
                .and_then(Value::as_array)
                .map(Vec::as_slice)
                .unwrap_or(&[]);
            let decoder = root
                .get("decoder_cases")
                .or_else(|| root.get("decode_cases"))
                .and_then(Value::as_array)
                .map(Vec::as_slice)
                .unwrap_or(&[]);
            (global, normal, decoder)
        }
        _ => return Err("dataset root must be an array or object".to_owned()),
    };

    let mut records = Vec::new();
    for (index, case) in normal_cases.iter().enumerate() {
        if is_decoder_case(case) {
            records.push(evaluate_decoder_case(case, index, &global_tools)?);
        } else {
            records.push(evaluate_normal_case(case, index, &global_tools)?);
        }
    }
    for (index, case) in decoder_cases.iter().enumerate() {
        records.push(evaluate_decoder_case(case, index, &global_tools)?);
    }
    Ok(records)
}

fn is_decoder_case(case: &Value) -> bool {
    let kind = case
        .get("type")
        .or_else(|| case.get("case_type"))
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_ascii_lowercase();
    let id = case
        .get("id")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_ascii_lowercase();
    kind.contains("decoder")
        || kind.contains("stream")
        || id.starts_with("dc-")
        || case.get("chunks").is_some()
        || case.get("input_chunks").is_some()
}

fn evaluate_normal_case(
    case: &Value,
    index: usize,
    global_tools: &[ToolDef],
) -> Result<Value, String> {
    let id = case_id(case, index, "case");
    let tools = case_tools(case, global_tools)?;
    let compact = encode_tools(&tools).map_err(|e| format!("case {id}: {e}"))?;
    let expected = expected_calls(case);
    let rendered = render_calls(&expected)?;
    let decoded = decode_calls(&rendered, &tools);
    let (roundtrip, decode_error) = match decoded {
        Ok(calls) => (to_openai_values(&calls)?, None),
        Err(error) => (Vec::new(), Some(error.to_string())),
    };
    let mut record = Map::new();
    record.insert("id".into(), json!(id));
    record.insert(
        "compact_request".into(),
        json!({
            "instructions": CALL_GRAMMAR,
            "compact_tools": compact.rendered,
            "bypassed_tools": bypasses(&compact),
        }),
    );
    record.insert("compacted".into(), json!(all_compacted(&compact)));
    record.insert("rendered_calls".into(), json!(rendered));
    record.insert("roundtrip_calls".into(), Value::Array(roundtrip));
    if let Some(error) = decode_error {
        record.insert("decode_error".into(), json!(error));
    }
    Ok(Value::Object(record))
}

fn evaluate_decoder_case(
    case: &Value,
    index: usize,
    global_tools: &[ToolDef],
) -> Result<Value, String> {
    let id = case_id(case, index, "decoder");
    let tools = case_tools(case, global_tools)?;
    let chunks = decoder_chunks(case)?;
    let mut decoder = StreamDecoder::new(&tools);
    let mut all_calls: Vec<ToolCall> = Vec::new();
    let mut chunk_rows = Vec::new();
    let mut error = None;
    for (chunk_index, chunk) in chunks.iter().enumerate() {
        match decoder.push_chunk(chunk) {
            Ok(calls) => {
                all_calls.extend(calls.clone());
                chunk_rows.push(json!({
                    "index": chunk_index,
                    "chunk": chunk,
                    "state": format!("{:?}", decoder.state()),
                    "calls": calls,
                    "error": Value::Null,
                }));
            }
            Err(e) => {
                error = Some(e.to_string());
                chunk_rows.push(json!({
                    "index": chunk_index,
                    "chunk": chunk,
                    "state": format!("{:?}", decoder.state()),
                    "calls": [],
                    "error": e.to_string(),
                }));
                break;
            }
        }
    }
    if error.is_none() {
        match decoder.finish() {
            Ok(calls) => all_calls.extend(calls),
            Err(e) => error = Some(e.to_string()),
        }
    }
    let openai = to_openai_values(&all_calls)?;
    let mut decoded = Map::new();
    decoded.insert("chunks".into(), Value::Array(chunk_rows));
    decoded.insert("calls".into(), Value::Array(openai));
    decoded.insert("valid".into(), json!(error.is_none()));
    if let Some(error) = error {
        decoded.insert("error".into(), json!(error));
    }
    Ok(json!({
        "id": id,
        "decoded": Value::Object(decoded),
    }))
}

fn parse_tools(value: &Value) -> Result<Vec<ToolDef>, String> {
    let rows = match value {
        Value::Array(rows) => rows.iter().collect::<Vec<_>>(),
        Value::Object(map) if map.contains_key("function") || map.contains_key("parameters") => {
            vec![value]
        }
        Value::Object(map) => map.values().collect::<Vec<_>>(),
        _ => return Err("tools must be an array or object".to_owned()),
    };
    rows.into_iter()
        .enumerate()
        .map(|(index, row)| parse_tool(row, index))
        .collect()
}

fn parse_tool(value: &Value, index: usize) -> Result<ToolDef, String> {
    let function = value.get("function").unwrap_or(value);
    let name = function
        .get("name")
        .and_then(Value::as_str)
        .or_else(|| value.get("name").and_then(Value::as_str))
        .ok_or_else(|| format!("tool {index} has no name"))?;
    let parameters = function
        .get("parameters")
        .or_else(|| value.get("parameters"))
        .ok_or_else(|| format!("tool `{name}` has no parameters schema"))?;
    Ok(ToolDef {
        name: name.to_owned(),
        description: function
            .get("description")
            .or_else(|| value.get("description"))
            .and_then(Value::as_str)
            .map(str::to_owned),
        parameters: parameters.clone(),
    })
}

fn case_tools(case: &Value, global: &[ToolDef]) -> Result<Vec<ToolDef>, String> {
    for key in ["tools", "tool_definitions", "tool_defs", "tool_schema"] {
        if let Some(value) = case.get(key) {
            return parse_tools(value);
        }
    }
    if let Some(Value::Object(tool)) = case.get("tool") {
        if tool.get("parameters").is_some() || tool.get("function").is_some() {
            return parse_tools(&Value::Object(tool.clone()));
        }
    }
    if !global.is_empty() {
        return Ok(global.to_vec());
    }
    Err("case has no tools and dataset has no global tools".to_owned())
}

fn case_id(case: &Value, index: usize, prefix: &str) -> String {
    case.get("id")
        .and_then(Value::as_str)
        .map(str::to_owned)
        .unwrap_or_else(|| format!("{prefix}-{:03}", index + 1))
}

fn expected_calls(case: &Value) -> Vec<ToolCall> {
    let list = case
        .get("expected_calls")
        .or_else(|| case.get("expected_tool_calls"))
        .or_else(|| case.get("tool_calls"))
        .or_else(|| case.get("calls"))
        .or_else(|| case.get("expected_call"))
        .or_else(|| case.get("expected"));
    if let Some(Value::Array(rows)) = list {
        return rows.iter().filter_map(parse_call).collect();
    }
    if let Some(Value::Object(row)) = list {
        return parse_call(&Value::Object(row.clone()))
            .into_iter()
            .collect();
    }
    parse_call(case).into_iter().collect()
}

fn parse_call(value: &Value) -> Option<ToolCall> {
    let function = value.get("function").unwrap_or(value);
    let name = function
        .get("name")
        .or_else(|| function.get("tool"))
        .or_else(|| function.get("tool_name"))?
        .as_str()?;
    let arguments = function
        .get("arguments")
        .or_else(|| function.get("args"))
        .or_else(|| value.get("arguments"))
        .or_else(|| value.get("args"))?;
    let arguments = match arguments {
        Value::String(text) => serde_json::from_str(text).ok()?,
        value => value.clone(),
    };
    Some(ToolCall {
        name: name.to_owned(),
        arguments,
    })
}

fn render_calls(calls: &[ToolCall]) -> Result<String, String> {
    calls
        .iter()
        .map(|call| {
            if !call.arguments.is_object() {
                return Err(format!(
                    "expected arguments for `{}` to be a JSON object",
                    call.name
                ));
            }
            let arguments = serde_json::to_string(&call.arguments).map_err(|e| e.to_string())?;
            Ok(format!("<<call {} {}>>", call.name, arguments))
        })
        .collect::<Result<Vec<_>, _>>()
        .map(|rows| rows.join("\n\n"))
}

fn decoder_chunks(case: &Value) -> Result<Vec<String>, String> {
    let chunks_value = case.get("chunks").or_else(|| case.get("input_chunks"));
    if let Some(Value::Array(values)) = chunks_value {
        return values
            .iter()
            .map(|v| match v {
                Value::String(s) => Ok(s.clone()),
                Value::Object(_) => v
                    .get("text")
                    .or_else(|| v.get("content"))
                    .and_then(Value::as_str)
                    .map(str::to_owned)
                    .ok_or_else(|| "decoder chunk object needs text/content".to_owned()),
                _ => Err("decoder chunks must be strings or objects with text".to_owned()),
            })
            .collect();
    }
    let source = case
        .get("text")
        .or_else(|| case.get("input"))
        .or_else(|| case.get("model_output"))
        .or_else(|| case.get("output"))
        .or_else(|| case.get("response"))
        .or_else(|| case.get("rendered_calls"))
        .and_then(Value::as_str)
        .map(str::to_owned)
        .or_else(|| {
            let generated = expected_calls(case);
            if generated.is_empty() {
                None
            } else {
                render_calls(&generated).ok()
            }
        })
        .ok_or_else(|| {
            "decoder case needs `chunks`, `text`, `input`, or expected calls".to_owned()
        })?;
    // Fixed-size character chunks are generated from the input, not hand-authored
    // per case. This naturally splits markers, tool names, strings, and JSON.
    let chars: Vec<char> = source.chars().collect();
    Ok(chars
        .chunks(7)
        .map(|chunk| chunk.iter().collect())
        .collect())
}

fn to_openai_values(calls: &[ToolCall]) -> Result<Vec<Value>, String> {
    calls
        .iter()
        .enumerate()
        .map(|(index, call)| {
            let converted = call
                .to_openai(format!("call_{}", index + 1))
                .map_err(|e| e.to_string())?;
            serde_json::to_value(converted).map_err(|e| e.to_string())
        })
        .collect()
}

fn bypasses(compact: &CompactTools) -> Vec<Value> {
    compact
        .tools
        .iter()
        .filter_map(|tool| match &tool.decision {
            nasiko_tool_compact::CompactionDecision::Bypass { reason } => {
                Some(json!({"name":tool.name,"reason":reason}))
            }
            nasiko_tool_compact::CompactionDecision::Compact { .. } => None,
        })
        .collect()
}

fn all_compacted(compact: &CompactTools) -> bool {
    !compact.tools.is_empty() && compact.tools.iter().all(|tool| tool.decision.is_compact())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn sample_tool() -> Value {
        json!({"type":"function","function":{"name":"greet","description":"Say hello","parameters":{"type":"object","properties":{"name":{"type":"string"}},"required":["name"],"additionalProperties":false}}})
    }

    #[test]
    fn handles_array_and_split_decoder_cases_in_stable_order() {
        let dataset = json!({
            "tools":[sample_tool()],
            "cases":[{"id":"normal","tool":"greet","arguments":{"name":"Mira"}}],
            "decoder_cases":[{"id":"decoder","text":"<<call greet {\"name\":\"Mira\"}>>"}]
        });
        let out = evaluate_dataset(&dataset).unwrap();
        assert_eq!(out.len(), 2);
        assert_eq!(out[0]["id"], "normal");
        assert_eq!(out[0]["roundtrip_calls"][0]["function"]["name"], "greet");
        assert_eq!(out[1]["decoded"]["valid"], true);
        assert!(!out[1]["decoded"]["chunks"].as_array().unwrap().is_empty());
    }

    #[test]
    fn decoder_case_reports_incomplete_input() {
        let dataset =
            json!({"tools":[sample_tool()],"decoder_cases":[{"id":"partial","chunks":["<<ca"]}]});
        let out = evaluate_dataset(&dataset).unwrap();
        assert_eq!(out[0]["decoded"]["valid"], false);
        assert!(out[0]["decoded"]["error"]
            .as_str()
            .unwrap()
            .contains("incomplete"));
    }
}
