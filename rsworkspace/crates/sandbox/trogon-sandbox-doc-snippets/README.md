# trogon-sandbox-doc-snippets

Compiles the Rust code in the sandbox documentation, so a page that stops matching the public sandbox API fails
the tests instead of failing the reader.

The library is empty. It has one module per documentation page, and each module file takes the page as its
doc comment with `include_str!`. Rustdoc then treats every `rust` block on the page as a doc test and compiles it
against `trogon-sandbox-client`, `trogon-sandbox-contract`, and `trogon-sandbox-protocol`. Blocks marked
`rust,no_run` are compiled but not run, because they need a live broker and worker.

Run the checks from `rsworkspace/`:

```sh
cargo test -p trogon-sandbox-doc-snippets --doc
```

| Module | Page |
|---|---|
| `first_sandbox_lease` | `docs/get-started/sandbox/first-sandbox-lease.md` |

To cover another page, add a module file whose doc comment is `include_str!` of the page, add the page to the `sandbox_doc_snippets`
filegroup in `docs/BUILD.bazel` so Bazel can read it, and add a row to the table above.
