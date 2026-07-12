# AI8888 Tools

AI8888 Tools is a Tauri desktop client for managing AI8888 API configuration across local coding tools. It can synchronize account, subscription, group, and API key information, write local configuration for Codex, Claude Code, OpenCode, OpenClaw, and Hermes, manage local routing, browse and resume local Codex sessions, and check for application updates from GitHub Releases.

Current version: v0.0.3

## Features

- Account login and API key management for AI8888.
- Multi-tool configuration writing for Codex, Claude Code, OpenCode, OpenClaw, and Hermes.
- Endpoint probing across AI8888 domains for regions where some domains may be blocked.
- Standalone Codex session manager with browsing, multi-select resume, and session visibility repair.
- GitHub Releases based update check from the main window footer.
- Mainland China download acceleration for GitHub release assets when the exit IP is CN.

## Packaging policy

Local machines only write and push code.

GitHub Actions builds Windows / Linux / macOS installers and publishes GitHub Releases.

### Release flow

1. Commit and push code to `master`.
2. Create and push a version tag, for example:

```bash
git tag v0.0.3
git push origin v0.0.3
```

3. GitHub Actions workflow `Build and release desktop packages` will:
- build Windows / Linux / macOS packages
- upload artifacts
- create/update the GitHub Release for that tag

You can also run the workflow manually from the Actions tab.
If you provide a tag input there, it will both build and publish.
If you leave the tag empty, it only builds artifacts.

## Open Source Acknowledgements

This project thanks and references ideas from these open source projects:

- cockpit-tools: https://github.com/jlcodes99/cockpit-tools
- cc-switch: https://github.com/jlcodes99/cc-switch
- sub2api: https://github.com/Wei-Shaw/sub2api

## Development

```bash
npm install
npm run typecheck
cargo check -q --manifest-path src-tauri/Cargo.toml
npm run build:renderer
```