# Architecture

ruby-lsp-rs is a port of [Shopify's ruby-lsp](https://github.com/Shopify/ruby-lsp). Each Rust module mirrors a Ruby file
so that behavior can be compared side by side.

| Rust | ruby-lsp |
| --- | --- |
| `src/server.rs` | `base_server.rb`, `server.rb` |
| `src/global_state.rs` | `global_state.rb`, `client_capabilities.rb`, `RequestConfig` |
| `src/document.rs` | `document.rb`, `ruby_document.rb`, `erb_document.rb` |
| `src/store.rs` | `store.rb` |
| `src/node_context.rs` | `node_context.rb`, `RubyDocument.locate` |
| `src/type_inferrer.rs` | `type_inferrer.rb` |
| `src/requests/<name>.rs` | `requests/<name>.rb` |
| `src/requests/support/` | `requests/support/common.rb`, `Request` helpers, `SorbetLevel`, `Interface` |
| `src/listeners/<name>.rs` | `listeners/<name>.rb` |
| `src/indexer/` | `ruby_indexer` |

## Message processing

- `main.rs` runs `Server::run` over stdio (`lsp-server`).
- The main loop processes one message at a time. Before processing, every message already received is drained into a
  queue so that `$/cancelRequest` can cancel queued requests (answered with `RequestCancelled`).
- `initialize` detects the Ruby environment by running `ruby/environment.rb` with the `rubyCommand` initialization option
  (default `["ruby"]`), because formatter, linter, test library and Sorbet detection need the bundle's dependencies.
- `initialized` builds the index on a background thread (`$/progress` is reported when the client supports it). The new
  index replaces the live one once done, re-indexing documents that were indexed from unsaved editor state.
- Each request handler runs inside `catch_unwind`: a panic answers `InternalError`, so the editor never hangs.
  `RequestError::InvalidParams` is used for missing documents and invalid positions, like ruby-lsp.
- Unknown methods answer `MethodNotFound`.

## Type checking

The type checker is a Rust port of Steep, split into crates that mirror Steep and the rbs gem:

| Crate | Ruby |
| --- | --- |
| `crates/rbs-definition` | rbs gem: `Environment`, `AncestorBuilder`, `DefinitionBuilder`, loaders, inline RBS, rbs collection |
| `crates/steep-interface` | Steep: `AST::Types`, `Interface::Builder`, `Subtyping::Check` |
| `crates/steep` | Steep: `Project::DSL`, the parser gem AST, `TypeConstruction`, `Typing`, diagnostics, `Services::*` |

The server uses the services of `crates/steep` as Steep's language server uses them, through `src/type_checking/`:

| Rust | Steep |
| --- | --- |
| `src/type_checking/worker.rs` | `Server::TypeCheckWorker`, `Server::InteractionWorker` |
| `src/type_checking/lsp_formatter.rs` | `Server::LSPFormatter`, `Diagnostic::LSPFormatter` |
| `src/type_checking/inlay_hints.rs` | none: inlay hints are built from the node types of a `Typing` |
| `src/type_checking/mod.rs` | the settings (`typeChecker`) and the client the requests query |

- `TypeCheckService` keeps `Rc` values, so one thread (`worker.rs`) owns it. `initialized` starts the thread and sends
  `Command::Load`: the thread reads the `Steepfile` without running Ruby (a single `default` target checking `.` with
  signatures in `sig` when there is none), decides the Steep version from `Gemfile.lock` (`steep_gem.rs`), finds the rbs
  4.2.0 gem, loads the signatures of every target once, and answers `Event::Loaded`. Until then the requests answer
  without type information.
- `didOpen` and `didChange` send the whole text of `.rb` and `.rbs` documents as `Command::SetText`, applied as a
  `ContentChange`. A Ruby file is type checked again on the next query. A change of a `.rbs` file or of a file with inline
  RBS reloads the signatures of the targets that read it, 300 ms after the last change or before the next query, and
  sends `Event::SignaturesChanged` so that the server refreshes diagnostics and inlay hints. Files changed on disk
  (`rbs-watcher`, `workspace-watcher`) are read again the same way; a change of the `Steepfile` or of the rbs collection
  files rebuilds the service.
- Requests send a `Query` with a channel for the answer and wait for it. Answers are owned, `Send` values with LSP
  positions in the negotiated encoding: diagnostics as LSP JSON, hover markdown and range, completion items, signature
  help, location links and type hints.
- While idle, the thread type checks every file of the project so that `GotoService` finds definitions in Ruby files, as
  Steep's type check workers do. A goto query checks the files left first, for at most two seconds. Without a
  `Steepfile` only the opened files are checked.
- Each request merges the answer with ruby-lsp's own result: hover puts the type checker's markdown above ruby-lsp's,
  completion lists the type checker's items first and drops ruby-lsp's items with a guessed receiver or the same label,
  and definition prefers the implementations the type checker found, then its signatures, then ruby-lsp's name based
  results, following `typeChecker.definition.target`.
- `typeChecker.inferMoreThanSteep` (off by default) sends `Command::SetInferMoreThanSteep`, which sets
  `TypeCheckService::inference_rules`. Hover, completion, signature help, goto and inlay hints then type check with the
  rules of `crates/steep/src/inference.rs` on top of Steep's typing, while `TypeCheckService::typecheck_source`, which
  gives the diagnostics, always type checks as Steep does. A file is therefore checked once for its diagnostics and once
  more for each query, as before; only the types of the queries change. Each rule is applied in one named place
  (`type_construction/inference_rules.rs`, and the rewrite of `it` blocks for Steep 2.0 in `inference.rs`) and records the
  node it applied to in `Typing`. `crates/steep/tests/inference_rules.rs` checks the end-to-end cases and the fixtures
  with both modes: the diagnostics are identical, and every node typed differently is explained by a rule.

## Adding a request

Every method of ruby-lsp's `Server#process_message` is already registered in `requests::handler_for` and has its own
module in `src/requests/`. Unimplemented modules return `null`. To implement a request, only edit its module (and add a
listener in `src/listeners/` when it mirrors one). The handler signature is:

```rust
pub fn handle(context: &mut RequestContext<'_>, params: Value) -> RequestResult
```

Typical shape, taken from `src/requests/definition.rs`:

```rust
let params: TextDocumentPositionParams = parse_params(params)?;      // or TextDocumentParams, TextDocumentRangeParams
let document = context.store.get(&params.text_document.uri)?;      // reads from disk if not open, like Store#get
if !document.is_ruby_or_erb() {
    return Ok(Value::Null);                                          // ruby-lsp's send_empty_response
}
let global_state = &*context.global_state;
let parsed = document.parse();                                      // Prism parse result + LineIndex
let offset = parsed.offset_of(params.position)?;                     // editor position -> byte offset
let node_context = parsed.locate(offset, &["CallNode", "ConstantReadNode"]); // RubyDocument.locate
let sorbet_level = SorbetLevel::for_document(global_state, &parsed);
let mut index = global_state.index();                               // MutexGuard<Index>
let receiver = TypeInferrer::new(&mut index).infer_receiver_type(&node_context);
to_result(response)                                                  // any Serialize value
```

Conventions:

- Node types are referred to by their Prism class name (`crate::prism::node_type_name`), e.g. `"CallNode"`.
- Offsets are bytes. `parsed.range_from_node` and `parsed.range_from_location` produce ranges with byte columns, which is
  what ruby-lsp's `range_from_node` does (Prism columns are bytes). `parsed.code_units_range` uses the negotiated
  encoding. Index entry locations are converted with `support::common::range_from_location`.
- Use `support::interface::{Location, LocationLink, LocationOrLink}` for responses: their URIs are plain strings, so
  any URI the client sent round-trips unchanged.
- Results that ruby-lsp caches per document can use `document.cache_get` / `cache_set` / `cache_fetch`; the cache is
  cleared on every edit.
- The automatic requests (`textDocument/documentSymbol`, `documentLink`, `codeLens`, `foldingRange`) re-index the
  document before the handler runs, like `Server#run_combined_requests`. Handlers don't have to do it.
- `context.outgoing` sends notifications (`log_message`, `show_message`, `notify`).
- Return `RequestError::Delegate` (via `support::common::delegate_request_if_needed`) for ERB positions that belong to
  the host language.

## Expectations tests

`tests/expectations_test.rs` mirrors `test/requests/support/expectations_test_runner.rb`. The fixtures and expectations
are copied from ruby-lsp into `tests/fixtures` and `tests/expectations` (MIT, see `tests/fixtures/LICENSE-ruby-lsp`).
For each registered request kind, every fixture runs through the real server:

- with `tests/expectations/<kind>/<fixture>.exp.json`, the JSON `result` must equal the response, using the expectation's
  `params`
- without one, the request must succeed (`__does_not_raise`)

To add a kind, write a runner and register it:

```rust
fn run_hover(case: &ExpectationCase) -> Result<Value, String> {
    let mut server = with_server(Some(&case.source), DEFAULT_URI, TestOptions { stub_no_typechecker: true, ..TestOptions::default() });
    let position = case.params.as_ref().and_then(|params| params.get(0)).cloned().unwrap_or(json!({ "line": 0, "character": 0 }));
    let response = server.request("textDocument/hover", json!({ "textDocument": { "uri": DEFAULT_URI }, "position": position }));
    response_result(response)
}

const REQUEST_KINDS: &[RequestKind] = &[
    RequestKind { expectation_directory: "definition", run: run_definition },
    RequestKind { expectation_directory: "hover", run: run_hover },
];

#[test]
fn hover_expectations() {
    run_expectations(kind("hover"));
}
```

`WRITE_EXPECTATIONS=1 cargo test --test expectations_test` rewrites mismatching expectation files, like ruby-lsp.

`tests/support/mod.rs` is the equivalent of ruby-lsp's `with_server`: it starts a real `Server` on a thread connected
through `lsp_server::Connection::memory()`, opens and indexes the source, and offers `request`, `result`, `notify`,
`definition`, `index()` and `index_source`. Ported request tests (like `tests/definition_test.rs`) use it directly.
Ruby LSP's own bundle depends on Sorbet, so `with_server` runs with a type checker unless `stub_no_typechecker` is set,
exactly like the Ruby tests.
