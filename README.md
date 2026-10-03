# CompactCall · Nasiko

A dependency-free browser prototype for exploring compact tool schemas and compact-call decoding.

## Run locally

Requires Node.js 18 or newer. No package installation is needed.

```sh
npm start
```

Open [http://localhost:4173](http://localhost:4173).

## Included views

Workspace, Schema Audit, Stream Simulator, Compression Report, Adherence Lab, Evaluation, Run History, Settings, Tool Catalog, Schema Editor, Failure Inspector, Security Report, Model Effectiveness, IAM and Roles, and Organization Overview.

The core workspace supports JSON import/paste/edit, compact signature inspection, local character estimates, compact-call decoding, client-side validation for its supported schema subset, and OpenAI-compatible output inspection. The stream simulator splits text into configurable chunks. Evaluation can validate pasted case arrays locally and export a JSONL preview.

## Prototype boundaries

- This folder contains a browser UI; it does **not** contain the PRD's `nasiko-tool-compact` Rust crate or `compact_tools_eval.rs` evaluation example.
- Validation runs in browser JavaScript and supports a limited schema subset. It is not a complete JSON Schema validator. Unrecognized keywords and unsupported shapes are bypassed in the UI.
- Character counts and compression percentages are estimates. No tokenizer is connected, so they are not token measurements.
- Evaluation preview does not call a model. Adherence and model effectiveness show empty or requirement-only states, not measured results.
- Organization counts, personas, and IAM permissions are UI examples. They are not backed by identity, persistence, or server authorization.
- Schemas and model text remain in the browser tab. The included Node server only serves static files.
