# AI8888 Tools

AI8888 Tools is a Tauri desktop client for managing AI8888 API configuration across local coding tools. It can synchronize account, subscription, group, and API key information, write local configuration for Codex, Claude Code, OpenCode, OpenClaw, and Hermes, manage local routing, browse and resume local Codex sessions, and check for application updates from GitHub Releases.

Current version: v0.0.6

## Features

- Account login and API key management for AI8888.
- Multi-tool configuration writing for Codex, Claude Code, OpenCode, OpenClaw, and Hermes.
- Endpoint probing across AI8888 domains for regions where some domains may be blocked.
- Standalone Codex session manager with browsing, multi-select resume, and session visibility repair.
- GitHub Releases based update check from the main window footer.
- Mainland China download acceleration for GitHub release assets when the exit IP is CN.
- Streamed update download/install with progress, cancellation, accelerated-download fallback, digest verification, and failed-download cleanup.
- Transactional multi-tool configuration writes with automatic failure rollback and up to 20 versioned snapshots.
- Reusable configuration Profiles for endpoint, Key, model, target tool, and local-routing rules, with one-click transactional apply.
- Account usage and subscription expiry alerts with dismiss actions (auto-refresh every 15 minutes while logged in, plus on window focus).
- Codex session full-text search with provider/archive filters.
- First-run setup wizard for endpoint probe, tool/key selection, and config write.

## Packaging policy

Local machines only write and push code.

GitHub Actions verifies every `master` push, builds Windows / Linux / macOS installers, and publishes GitHub Releases only for version tags.

### Release flow

1. Commit and push code to `master`. GitHub Actions runs tests, checks, and cross-platform builds without publishing a release.
2. Create and push a version tag, for example:

```bash
git tag v0.0.6
git push origin v0.0.6
```

3. GitHub Actions workflow `Build and release desktop packages` will:
- build Windows / Linux / macOS packages
- upload artifacts
- verify MSI, NSIS, universal DMG, AppImage, deb, rpm, and GitHub SHA-256 digests
- create a draft Release and publish it only after all asset checks pass

Manual workflow runs only build artifacts and never publish. Release tags must exactly match the version embedded in npm, Cargo, Tauri, Rust, and the renderer.

macOS signing and notarization are enabled automatically when the repository provides all `APPLE_*` signing secrets; otherwise the workflow produces an unsigned DMG.

## Open Source Acknowledgements

This project thanks and references ideas from these open source projects:

- cockpit-tools: https://github.com/jlcodes99/cockpit-tools
- cc-switch: https://github.com/jlcodes99/cc-switch
- sub2api: https://github.com/Wei-Shaw/sub2api

## Development

Local machines edit, commit, and push source code. Compilation, tests, packaging, and release publication run in GitHub Actions.
