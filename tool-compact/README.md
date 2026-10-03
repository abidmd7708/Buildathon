# nasiko-tool-compact

Pure Rust library for deterministic tool-schema compaction and fail-closed
`<<call tool_name {"arg":1}>>` decoding. It has no router, provider, filesystem,
environment, or network dependency. Original schemas are retained in
`CompactTools` and remain the validation source of truth.

## Schema policy

The encoder supports object properties and required lists; `string`, `integer`,
`number`, `boolean`, `array`, and nested object types; primitive enums; field and
tool descriptions; and string formats `date`, `date-time`, and `email`.
Optional properties receive `?`; required properties do not. Descriptions are
kept after the type when provided. Objects that permit extra properties include
an explicit `...:json` signature marker.

Unsupported schema keywords are bypassed with the first deterministic JSON path
and reason. This includes (among others) `oneOf`, `anyOf`, `allOf`, `$ref`,
`pattern`, `minimum`, `maxLength`, and conditional keywords. `additionalProperties`
with a schema value is bypassed. Unknown keywords are never discarded. Bypassed
tools cannot be decoded by this library.

The built-in validator enforces the supported subset: exact JSON types, required
fields, enums, nested values, formats, and `additionalProperties: false`. It does
not coerce or repair values. JSON Schema features outside the supported subset
are rejected at analysis time.

## Grammar and streaming

```text
CALL        := OPEN TOOL_NAME WS JSON_OBJECT CLOSE
OPEN        := "<<call "
TOOL_NAME   := exact non-whitespace name
JSON_OBJECT := valid JSON object
CLOSE       := ">>"
```

Prose before, between, and after calls is accepted. JSON string escapes and
nested arrays/objects are scanned before recognizing the closing marker. An
unknown tool, malformed marker/JSON, invalid argument, or unfinished stream
returns a structured error. `StreamDecoder::push_chunk` returns each newly
completed call once; `finish` rejects incomplete markers or JSON.

## Tests and evaluator

```sh
cargo test -p nasiko-tool-compact
EVAL_SET=/tmp/compact-tools-eval.json OUT=/tmp/out.jsonl \
  cargo run --release -p nasiko-llm-router --example compact_tools_eval
```

The example accepts a top-level case array, or an object with `tools` and
`cases`/`normal_cases`, plus optional `decoder_cases`. Cases may define their own
`tools`/`tool_definitions`; expected calls can use `expected_calls`,
`expected_tool_calls`, `tool_calls`, or `calls`. Decoder cases accept `chunks`,
`input_chunks`, `text`, `input`, `model_output`, or `rendered_calls`. If text is
provided, the example deterministically generates seven-character chunks.

`EVAL_SET` and `OUT` are required by the example only. Evaluation is offline and
deterministic. The evaluator does not call a model and does not claim measured
token savings or model adherence. Tokenizer integration and optional live mode
are not implemented in this checkout.
