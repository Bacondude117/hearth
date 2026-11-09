# Hearth — notes for contributors and agents

- Logic belongs in `crates/hearth-core` (UI-agnostic, tested). `src-tauri` stays a thin command layer.
- Core errors must read well to end users. They are shown verbatim in toasts.
- Game-specific behavior goes behind `GameProvider`. Avoid `if game == ...` outside providers.
- Downloads: always go through `http::download` with a checksum when the source publishes one.
- Tests: `cargo test --workspace`. Process tests use `tests/fake_server.sh` (Unix only). Live
  network tests are `#[ignore]`d.
- UI: `npm run dev` runs the interface in a browser against `ui/src/mock.ts`. Keep the mock in
  step when commands change.

**Browser support:** the UI targets WebView2 (evergreen Chromium) on Windows. Baseline Newly
Available features may be used. Keep a small fallback only where Linux dev builds (WebKitGTK)
would otherwise break core flows (e.g. the `<dialog closedby>` light-dismiss fallback in `Modal.tsx`).
