# hello-hook

Minimal native DeepSeekCode plugin example. The desktop/engine also ships a built-in
`NativeSystemEchoHook` with the same id for MVP hook demos.

Build from this directory:

```bash
cargo build --release
```

The output `target/release/hello_hook.dll` (Windows) or `.so` / `.dylib` can be
referenced from `deepseekcode.jsonc` `plugins[]` or placed under `.deepseekcode/plugins/`.
