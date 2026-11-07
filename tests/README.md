# Synthetic UI regression fixtures

## Episode review and layout

`episode-ui.html?mode=success&auto=1` tests a real component click with mocked IPC. Other modes: `failure`, `dirty` (save then review), `legacy` (no review request), `unresolved` (candidate is not called passed), and `layout` (switch evidence/delivery tabs, check overflow). Layout was checked at 980px and 620px browser widths. No provider requests, task reads or mutations occur. Fixture uses `/src/App.css` and synthetic Chinese story text.

The review button must show a running label immediately, submit once under double-click, surface failures near the toolbar, and save dirty text before reviewing the returned revision. This verifies browser/React interaction, not a real DeepSeek review or native WebView/provider end-to-end result.

## Migration

`migration-ui.html` / `migration-ui.tsx` load only a synthetic migration component and mock Tauri IPC. They never read task data or call AI. Vite serves them in development; they are not imported by the production app.

With `npm run dev` running on port 1422, open:

- `/tests/migration-ui.html?mode=success&auto=1`
- `/tests/migration-ui.html?mode=failure&auto=1`
- `/tests/migration-ui.html?mode=stale&auto=1`
- `/tests/migration-ui.html?mode=status-error&auto=1`

Each page appends `#test-result` with PASS when the expected interaction completes. Success/failure require checking the explicit confirmation, clicking the button and observing the reply. A second immediate click must not duplicate the migration request. Stale-material and status-error cases must never submit migration. Run with a local headless Chrome `--dump-dom --virtual-time-budget=8000` if automated browser tooling is unavailable. These are synthetic interaction checks, not migration of a real task.
