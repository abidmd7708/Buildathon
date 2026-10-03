# PRD — Nasiko P1: Compact Tool Schemas Without Breaking Tool Calls

**Project:** Nasiko OSS Contribution  
**Track:** P1 — Compact tool schemas without breaking tool calls  
**PR title prefix:** `[compact-tools]`  
**Status:** Implementation-ready  
**Primary goal:** Reduce prompt-token overhead from OpenAI-style tool definitions while preserving tool semantics, safely decoding model-generated compact calls, and validating every call against the original JSON Schema.

---

## 1. Product Summary

Build a standalone Rust library, `nasiko-tool-compact`, plus the required Nasiko evaluation example.

The system takes native OpenAI-style tool definitions, determines whether each schema can be safely compacted, converts supported schemas into short LLM-friendly signatures, injects explicit compact-call instructions, decodes compact calls from normal or streamed model output, validates arguments against the **original full JSON Schema**, and converts valid calls into standard OpenAI-compatible tool calls.

Unsupported or risky schemas must be **bypassed**, not approximated.

Existing Nasiko router behavior must remain unchanged unless compact mode is explicitly enabled.

---

## 2. Source Requirements

This PRD is based on the official participant brief supplied for this project.

The brief requires:

- A new root crate `tool-compact/` with package `nasiko-tool-compact`.
- A pure library with no I/O, environment reads, or provider-specific code.
- The library must not depend on `nasiko-llm-router`.
- Required functionality equivalent to:
  - `encode_tools(&[ToolDef]) -> Result<CompactTools>`
  - `decode_calls(text: &str, &[ToolDef]) -> Result<Vec<ToolCall>>`
  - incremental `StreamDecoder`
- Optional `decode_tools(&CompactTools) -> Result<Vec<ToolDef>>`.
- A required `llm-router/examples/compact_tools_eval.rs`.
- Optional router wiring, disabled by default.
- Deterministic offline evaluation producing one JSONL line per case.
- Fail-closed validation.
- Support for required/optional fields, types, enums, nested objects and arrays.
- Explicit handling of malformed output and stream-split markers.
- Local token measurement with `tiktoken-rs`, `o200k_base`, as a dev-dependency of the example only.
- No API keys or sensitive data in the repository.

---

# 3. Goals

## 3.1 Primary Goals

1. Reduce tool-definition prompt tokens.
2. Preserve the semantic meaning of supported schemas.
3. Keep the original JSON Schema as the validation source of truth.
4. Provide an explicit and deterministic compact-call grammar.
5. Decode multiple compact tool calls.
6. Decode calls when markers and JSON are split across streaming chunks.
7. Validate every model-provided argument against the original schema.
8. Fail closed on invalid calls.
9. Safely bypass unsupported or risky schemas.
10. Produce standard OpenAI-compatible tool calls after successful validation.
11. Provide deterministic offline evaluation.
12. Make compaction decisions explainable.
13. Demonstrate token savings and round-trip correctness.

## 3.2 Secondary Goals

1. Support nested objects and arrays.
2. Preserve useful descriptions when they disambiguate fields.
3. Provide structured decoder/validation errors.
4. Add unit, property, streaming, and negative tests.
5. Make schema preservation automatically testable.
6. Keep the implementation easy to integrate into Nasiko later.

---

# 4. Non-Goals

The following are explicitly outside the core implementation:

- Replacing OpenAI's native tool-calling protocol.
- Gateway protocol changes.
- Direct provider interception.
- Changing default router behavior.
- Moving request classification into this project.
- Building an LLM-based schema analyzer.
- Automatically repairing malformed model arguments.
- Guessing missing arguments.
- Fuzzy matching tool names.
- Adding API keys or provider credentials.
- Building a production orchestration/agent framework.
- Optimizing unsupported JSON Schema features by changing their semantics.

Router integration is optional bonus work and must never be enabled by default.

---

# 5. Users / Consumers

## 5.1 Primary Consumer

Nasiko's LLM router.

## 5.2 Library Consumer

Future router/provider integrations that need to transform native tool definitions into a compact prompt representation and decode the resulting model output.

## 5.3 Evaluation Consumer

The official Nasiko evaluation harness, which executes:

```bash
cargo run --release -p nasiko-llm-router --example compact_tools_eval
```

---

# 6. Core User Journey

```text
Native OpenAI Tool Definitions
            |
            v
      Schema Analyzer
            |
       +----+----+
       |         |
    Supported  Risky/
       |       Unsupported
       v         |
Compact Form    Bypass
       |         |
       +----+----+
            |
            v
     Compact LLM Request
            |
            v
          Model
            |
            v
     Compact Text Output
            |
            v
      Stream Decoder
            |
            v
       Parsed Calls
            |
            v
 Original JSON Schema Validation
            |
       +----+----+
       |         |
     Valid     Invalid
       |         |
       v         v
OpenAI Call    Error
```

---

# 7. Compact Format

## 7.1 Canonical Call Syntax

The default grammar is:

```text
<<call tool_name {"argument":"value"}>>
```

Example:

```text
<<call create_calendar_event {"title":"Design review","start":"2026-10-05T15:00:00+05:30"}>>
```

The model may produce ordinary text before or after calls.

Example:

```text
I will schedule that.

<<call create_calendar_event {"title":"Design review","start":"2026-10-05T15:00:00+05:30"}>>

Done.
```

## 7.2 Multiple Calls

Calls are decoded in emitted order:

```text
<<call send_email {"to":["sam@example.com"],"subject":"Build status","body":"The build is green."}>>

<<call create_calendar_event {"title":"Retro","start":"2026-10-04T10:00:00+05:30","duration_min":30,"visibility":"private"}>>
```

## 7.3 JSON Is the Argument Grammar

Arguments must remain valid JSON.

Do not invent a second argument syntax.

This makes the compact format readable while allowing the original JSON Schema to remain authoritative.

---

# 8. Compact Tool Signature

A supported tool should be rendered approximately as:

```text
create_calendar_event(
  title:str,
  start:datetime,
  duration_min?:int,
  attendees?:[str],
  visibility?:public|private
) - Create an event in the user's calendar.
```

Required fields have no `?`.

Optional fields use `?`.

Examples:

```text
name:str
age?:int
active:bool
tags?:[str]
visibility:public|private
start:datetime
metadata?:{source?:str,priority?:int}
```

The exact formatting should be deterministic.

---

# 9. Schema Support Policy

## 9.1 Supported Features

The first implementation should support:

- `type: object`
- object `properties`
- `required`
- `type: string`
- `type: integer`
- `type: number`
- `type: boolean`
- `type: array`
- array `items`
- nested objects
- nested arrays
- `enum`
- supported string `format` values such as:
  - `date`
  - `date-time`
  - `email`
- field descriptions
- tool descriptions

## 9.2 Description Policy

Descriptions should be omitted when redundant.

Descriptions should be retained or shortened when necessary to disambiguate semantics.

Example:

```text
temperature:float — Celsius
```

instead of simply:

```text
temperature:float
```

if the description is semantically important.

## 9.3 Bypass Features

The implementation should safely bypass schemas containing unsupported or ambiguous features.

Examples include, unless explicitly implemented and tested:

- `oneOf`
- `anyOf`
- `allOf`
- conditional schemas
- complex `$ref` resolution
- recursive schemas
- unsupported combinators
- unsupported constraints whose semantics cannot be represented exactly

The implementation must never compact such schemas by silently dropping constraints.

## 9.4 Bypass Rule

If a schema cannot be represented without losing meaning:

```text
COMPACTION = BYPASS
```

not:

```text
COMPACTION = BEST_EFFORT
```

---

# 10. Adaptive Schema Analyzer

Create a deterministic analyzer.

Possible result:

```rust
pub enum CompactionDecision {
    Compact {
        reason: String,
    },
    Bypass {
        reason: String,
    },
}
```

The analyzer should inspect the schema and record useful metadata.

Example:

```text
Tool: create_calendar_event
Decision: COMPACT
Reason: supported object schema
Complexity: low
```

Example:

```text
Tool: complex_payment
Decision: BYPASS
Reason: unsupported oneOf constraint
Complexity: high
```

The decision must be deterministic:

```text
same schema -> same decision
```

Do not use an LLM to make this decision.

---

# 11. Complexity Metadata

The analyzer may compute deterministic complexity information such as:

```text
properties
nested_objects
arrays
enums
description_fields
unsupported_features
```

This is for diagnostics and explainability.

Complexity must not override correctness.

Example:

```text
HIGH complexity + fully supported
=> may still compact

UNSUPPORTED semantics
=> bypass
```

---

# 12. Data Model

The library must define its own tool types and must not depend on router types.

Suggested types:

```rust
pub struct ToolDef {
    pub name: String,
    pub description: Option<String>,
    pub parameters: serde_json::Value,
}

pub struct ToolCall {
    pub name: String,
    pub arguments: serde_json::Value,
}

pub struct CompactTools {
    // Original tool definitions / metadata
    // Deterministic compact rendering
    // Compaction decisions
}
```

The exact public API may differ, but it must provide the functionality required by the participant brief.

---

# 13. Required Public API

At minimum:

```rust
pub fn encode_tools(
    tools: &[ToolDef]
) -> Result<CompactTools>;

pub fn decode_calls(
    text: &str,
    tools: &[ToolDef]
) -> Result<Vec<ToolCall>>;
```

Streaming:

```rust
pub struct StreamDecoder {
    // incremental decoder state
}
```

Recommended:

```rust
pub fn decode_tools(
    compact: &CompactTools
) -> Result<Vec<ToolDef>>;
```

---

# 14. Stream Decoder

The decoder must be incremental.

It must correctly handle:

```text
chunk 1: <<ca
chunk 2: ll create_calendar_event {"title":"Ret
chunk 3: ro","start":"2026-10-04T10:00:00+05:30"}>
chunk 4: >
```

and reconstruct:

```text
<<call create_calendar_event {"title":"Retro","start":"2026-10-04T10:00:00+05:30"}>>
```

## 14.1 Required State

The implementation should track enough state to distinguish:

- ordinary text
- opening marker
- tool name
- JSON argument object
- JSON strings
- JSON escape sequences
- nested JSON objects/arrays
- closing marker

Do not detect `>>` merely by string search while inside a JSON string.

Example:

```json
{"text":"hello >> world"}
```

must remain valid JSON.

---

# 15. Decoder Grammar

Document an explicit grammar.

Conceptually:

```text
CALL        := OPEN TOOL_NAME WS JSON_OBJECT CLOSE
OPEN        := "<<call "
CLOSE       := ">>"
TOOL_NAME   := exact known tool name
JSON_OBJECT := valid JSON object
```

The parser must reject malformed markers.

The grammar must be implemented consistently in both full-text and streaming decoding.

---

# 16. Tool Name Validation

Tool names must match exactly.

Given:

```text
send_email
send_email_v2
send
```

the decoder must not use fuzzy or prefix matching.

Unknown tools must return:

```json
{"error":"unknown_tool"}
```

or an equivalent structured library error that the evaluation example converts to the required output.

---

# 17. Argument Validation

Every successful call must be validated against the original full JSON Schema.

Validation pipeline:

```text
Extract tool name
        |
        v
Exact tool lookup
        |
        v
Parse JSON
        |
        v
Original JSON Schema validation
        |
        v
Create ToolCall
```

## 17.1 Required Failures

Reject:

- unknown tool
- malformed marker
- malformed JSON
- missing required field
- wrong type
- invalid enum
- invalid nested value
- invalid array item
- unsupported call shape
- other JSON Schema violations supported by the implementation

## 17.2 No Repair

Never:

```text
"duraton_min" -> "duration_min"
```

Never:

```text
missing title -> infer title
```

Never:

```text
visibility:"secret" -> "private"
```

Never coerce:

```text
"30" -> 30
```

unless that coercion is explicitly part of the JSON Schema semantics being implemented.

The default behavior should be strict validation.

---

# 18. OpenAI-Compatible Conversion

After successful validation, convert the compact call to the standard OpenAI-compatible representation at the router seam.

Conceptually:

```json
{
  "tool_calls": [
    {
      "id": "call_1",
      "type": "function",
      "function": {
        "name": "create_calendar_event",
        "arguments": "{\"title\":\"Design review\",\"start\":\"2026-10-05T15:00:00+05:30\"}"
      }
    }
  ]
}
```

The compact library does not need to depend on router types.

The router is responsible for assigning the tool-call ID.

---

# 19. Error Model

Use structured library errors.

Suggested categories:

```rust
pub enum CompactError {
    InvalidSchema,
    UnsupportedSchema { reason: String },
    UnknownTool { name: String },
    MalformedMarker,
    MalformedJson,
    InvalidArguments { reason: String },
    MissingRequiredField { field: String },
    WrongType { field: String },
    InvalidEnum { field: String },
    SchemaViolation { reason: String },
    IncompleteStream,
}
```

The exact enum may vary.

Errors must be deterministic and must never result in a guessed tool call.

---

# 20. Round-Trip Schema Preservation

Implement:

```text
Original ToolDef
      |
      v
encode_tools()
      |
      v
CompactTools
      |
      v
decode_tools()
      |
      v
Recovered ToolDef
      |
      v
Semantic comparison
```

Schema comparison must not depend on JSON object key order.

The objective is to prove that compacting does not change the supported schema's meaning.

---

# 21. Evaluation Example

Create:

```text
llm-router/examples/compact_tools_eval.rs
```

The example must:

1. Read `EVAL_SET`.
2. Parse the public/private evaluation JSON.
3. Build compact requests.
4. Render expected calls into the compact grammar.
5. Decode the rendered calls.
6. Run decoder cases through `StreamDecoder`.
7. Optionally invoke a live OpenAI-compatible provider when configured.
8. Write exactly one JSONL output line per case.
9. Produce deterministic offline output.

Required environment variables:

```text
EVAL_SET
OUT
```

Optional live-mode variables:

```text
PROVIDER_BASE_URL
MODEL
```

No API keys should be committed.

---

# 22. Evaluation Output

Offline output should follow the required shape.

Example:

```json
{
  "id":"ct-001",
  "compact_request":{},
  "compacted":true,
  "rendered_calls":"<<call ...>>",
  "roundtrip_calls":[]
}
```

Decoder case example:

```json
{
  "id":"dc-002",
  "decoded":{
    "calls":[]
  }
}
```

Live mode may additionally include:

```json
{
  "raw_output":"...",
  "live_calls":[]
}
```

Do not output scores as the primary evaluation artifact; the official scorer calculates them.

---

# 23. Determinism

Running:

```bash
EVAL_SET=/tmp/compact-tools-eval.json \
OUT=/tmp/out1.jsonl \
cargo run --release -p nasiko-llm-router --example compact_tools_eval
```

and then:

```bash
EVAL_SET=/tmp/compact-tools-eval.json \
OUT=/tmp/out2.jsonl \
cargo run --release -p nasiko-llm-router --example compact_tools_eval
```

must produce equivalent output.

No:

- random IDs in offline output
- timestamps
- nondeterministic ordering
- random schema decisions

The router may assign runtime call IDs at the integration seam.

---

# 24. Token Measurement

Use:

```text
tiktoken-rs
encoding: o200k_base
```

as a dev-dependency of the evaluation example only.

Do not add the tokenizer dependency to the core library unless necessary.

Measure:

```text
token_savings =
1 - compact_tokens / baseline_tokens
```

The official scorer measures token reduction over the full request body.

Report per-tool and aggregate diagnostics locally where useful.

Example:

```text
create_calendar_event: 49.8%
send_email:             51.2%
overall:                50.4%
```

Bypassed cases count as zero savings for the official metric.

---

# 25. Testing Strategy

## 25.1 Unit Tests

Test:

- primitive types
- optional fields
- required fields
- enums
- arrays
- nested objects
- nested arrays
- descriptions
- date-time
- email
- multiple tools
- empty tools
- duplicate names
- unsupported schemas

## 25.2 Decoder Tests

Test:

- one call
- multiple calls
- text before call
- text after call
- text before and after
- no call
- unknown tool
- malformed marker
- malformed JSON
- incomplete call

## 25.3 Streaming Tests

Test:

```text
"<<ca" + "ll ..."
```

and:

```text
"...\"Ret" + "ro\"..."
```

and:

```text
"...}>" + ">"
```

Also test arbitrary chunk boundaries around:

- opening marker
- tool name
- JSON strings
- escaped characters
- nested objects
- arrays
- closing marker

## 25.4 Escaping Tests

Test JSON values containing:

```text
>>
<<
quotes
backslashes
newlines
unicode
```

Example:

```json
{
  "text":"Use >> carefully"
}
```

must decode correctly.

## 25.5 Validation Tests

Test:

```text
missing required
wrong string type
wrong number type
invalid enum
invalid nested field
invalid array item
extra fields where schema disallows them
```

## 25.6 Property Tests

For supported schemas:

```text
valid arguments
    -> encode/render
    -> decode
    -> validate
    -> same semantic arguments
```

For mutated invalid arguments:

```text
invalid arguments
    -> decode
    -> validation error
    -> never successful ToolCall
```

---

# 26. Security / Safety Invariants

The implementation must enforce:

### Invariant 1 — Original schema is authoritative

Compact representation is never the validation authority.

### Invariant 2 — Fail closed

Ambiguity or unsupported semantics result in bypass/error.

### Invariant 3 — No argument repair

Model-provided values are never silently changed.

### Invariant 4 — Exact tool names

No fuzzy tool matching.

### Invariant 5 — Strict JSON parsing

Malformed JSON is rejected.

### Invariant 6 — No hidden provider behavior

The library contains no provider API calls or credentials.

### Invariant 7 — No sensitive data

No API keys, private prompts, or user data in tests or committed files.

---

# 27. Router Integration — Optional Bonus

If implemented, compact mode must be explicitly opt-in.

Default:

```text
compact_mode = false
```

When false:

```text
existing behavior == existing behavior
```

When true:

```text
native tools -> compact representation
model output -> compact decoder
validated calls -> existing router ToolCall
```

The integration must be placed at the appropriate router seam and must not change unrelated routing behavior.

Before implementation, inspect existing:

```text
llm-router/src/brevity.rs
llm-router/src/compress.rs
compress/
llm-router/src/ir/chat.rs
binary config.rs
```

Avoid duplicating existing functionality.

---

# 28. Suggested Repository Structure

```text
nasiko/
├── Cargo.toml
│
├── tool-compact/
│   ├── Cargo.toml
│   └── src/
│       ├── lib.rs
│       ├── model.rs
│       ├── schema.rs
│       ├── compact.rs
│       ├── grammar.rs
│       ├── decoder.rs
│       ├── stream.rs
│       ├── validator.rs
│       └── error.rs
│
└── llm-router/
    └── examples/
        └── compact_tools_eval.rs
```

Suggested responsibility:

```text
model.rs       -> public ToolDef / ToolCall / CompactTools types
schema.rs      -> schema analysis and support detection
compact.rs     -> deterministic compact signature generation
grammar.rs     -> documented grammar/constants
decoder.rs     -> full-text decoding
stream.rs      -> incremental decoding
validator.rs   -> original JSON Schema validation
error.rs       -> structured errors
lib.rs         -> public API
```

---

# 29. Implementation Phases

## Phase 1 — Repository Inspection

- Inspect current workspace.
- Inspect `llm-router` tool types.
- Inspect existing compression/brevity utilities.
- Confirm Cargo workspace layout.
- Avoid duplicate functionality.

**Deliverable:** implementation notes and confirmed integration seam.

## Phase 2 — Core Types

Implement:

- `ToolDef`
- `ToolCall`
- `CompactTools`
- compaction decision metadata
- errors

**Deliverable:** compiling library.

## Phase 3 — Schema Analyzer

Implement:

- supported feature detection
- bypass detection
- deterministic complexity metadata
- safe decision

**Deliverable:** schema analyzer tests.

## Phase 4 — Compact Encoder

Implement:

- primitive type formatting
- optional/required formatting
- enum formatting
- arrays
- nested objects
- useful description retention
- tool descriptions
- deterministic output

**Deliverable:** compact definitions.

## Phase 5 — Decoder

Implement:

- exact tool lookup
- marker parsing
- JSON parsing
- multiple calls
- ordinary text handling
- structured errors

**Deliverable:** passing decoder tests.

## Phase 6 — StreamDecoder

Implement incremental state.

**Deliverable:** split-marker and split-JSON tests.

## Phase 7 — Validation

Implement original-schema validation.

**Deliverable:** negative/fail-closed tests.

## Phase 8 — Round Trip

Implement optional `decode_tools()` and semantic schema comparison.

**Deliverable:** schema preservation tests.

## Phase 9 — Evaluation

Implement:

```text
compact_tools_eval.rs
```

with:

- offline mode
- JSONL output
- decoder cases
- deterministic output
- optional live mode
- token measurement

## Phase 10 — Optional Router Wiring

Only after the core passes tests.

Add:

- explicit config flag
- default-off behavior
- conversion at router seam
- unchanged behavior test

---

# 30. Acceptance Criteria

## Core

- [ ] `tool-compact/` exists.
- [ ] Package is named `nasiko-tool-compact`.
- [ ] Library does not depend on `nasiko-llm-router`.
- [ ] No provider API calls in the library.
- [ ] No environment reads in the library.
- [ ] `encode_tools()` works.
- [ ] `decode_calls()` works.
- [ ] `StreamDecoder` works.

## Schema

- [ ] Required/optional semantics preserved.
- [ ] Primitive types preserved.
- [ ] Enums preserved.
- [ ] Arrays preserved.
- [ ] Nested objects preserved.
- [ ] Important descriptions preserved.
- [ ] Unsupported schemas bypassed.
- [ ] Bypass reason available.

## Decoder

- [ ] Multiple calls supported.
- [ ] Plain responses supported.
- [ ] Text before/after calls supported.
- [ ] Split markers supported.
- [ ] Split JSON supported.
- [ ] Escaped strings supported.
- [ ] `>>` inside JSON strings supported.
- [ ] Unknown tools rejected.

## Validation

- [ ] Required-field failures rejected.
- [ ] Wrong types rejected.
- [ ] Invalid enum values rejected.
- [ ] Malformed JSON rejected.
- [ ] Malformed markers rejected.
- [ ] No guessed calls.
- [ ] No silent argument repair.
- [ ] Original JSON Schema remains authoritative.

## Evaluation

- [ ] `compact_tools_eval.rs` exists.
- [ ] `EVAL_SET` supported.
- [ ] `OUT` supported.
- [ ] One JSONL line per case.
- [ ] Offline run requires no network/API key.
- [ ] Offline output is deterministic.
- [ ] Decoder cases are exercised chunk-by-chunk.
- [ ] Token measurement uses `o200k_base`.
- [ ] Live mode is optional and configurable.

## Router

- [ ] Default behavior remains unchanged.
- [ ] Compact mode is opt-in.
- [ ] Integration does not alter unrelated routing.

---

# 31. Demo Requirements

The demo should show at least:

## Demo A — Token Reduction

```text
Native schema
    ↓
Compact schema

Native:   XXXX tokens
Compact:  YYYY tokens
Savings:  ZZ.Z%
```

## Demo B — Successful Call

Input:

```text
<<call create_calendar_event {"title":"Design review","start":"2026-10-05T15:00:00+05:30"}>>
```

Output:

```text
OpenAI-compatible ToolCall
```

## Demo C — Multiple Calls

Show two calls decoded in order.

## Demo D — Streaming

Show the call arriving in several chunks and being reconstructed.

## Demo E — Failure

Show:

```text
visibility: "secret"
```

being rejected as an invalid enum.

## Demo F — Bypass

Show a schema with an unsupported feature being left uncompacted with a clear reason.

---

# 32. Performance Expectations

The implementation should be lightweight and deterministic.

Avoid:

- unnecessary allocations in the streaming path
- expensive schema recomputation for every call
- network calls in the core library
- model inference in the analyzer
- repeated compilation of the same schema

Schema analysis/encoding can happen once per request/tool set.

Validation happens per decoded call.

---

# 33. Observability / Diagnostics

The evaluation example should make it possible to inspect:

```text
tool name
compacted/bypassed
bypass reason
compact representation
round-trip result
decoder result
validation result
token counts
```

Do not expose sensitive user data.

---

# 34. PR Documentation

The PR description should contain:

```text
Track
How to run
Environment variables
Implementation summary
Supported schema features
Unsupported/bypassed features
Test strategy
Measured token reduction
Known limitations
Optional router integration status
```

Do not claim benchmark results that were not actually measured.

---

# 35. Definition of Done

P1 is considered implementation-complete when:

1. The standalone `nasiko-tool-compact` crate compiles.
2. Supported schemas compact deterministically.
3. Unsupported/risky schemas bypass safely.
4. Compact calls decode correctly.
5. Streaming chunks decode correctly.
6. Multiple calls decode correctly.
7. Original JSON Schema validates every call.
8. Invalid calls fail without repair or guessing.
9. Schema round-trip tests pass.
10. Required evaluation example runs offline.
11. Evaluation output is deterministic JSONL.
12. Token measurement is implemented.
13. Required public decoder cases pass.
14. Tests cover both success and failure paths.
15. Router behavior remains unchanged by default.
16. The PR is scoped to the permitted files/crates and contains no secrets.

---

# 36. Design Principle

The central design rule for the project is:

> **Compact the description, never compact away the contract.**

The compact format is an LLM-facing representation.

The original JSON Schema is the authoritative contract.

Therefore:

```text
Compact representation
        ≠
Validation authority

Original JSON Schema
        =
Validation authority
```

This allows aggressive token reduction where safe while preserving correctness and fail-closed behavior.
