# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.19.0](https://github.com/IvanWng97/pixtuoid/compare/v0.18.0...v0.19.0) - 2026-09-21

### Bug Fixes

- *(deps)* remove ttf-parser from the graph, retiring the last deny ignore ([#980](https://github.com/IvanWng97/pixtuoid/pull/980))
- *(dsh)* a truthy non-list insert row is refused loud; null passes as upstream's own no-op ([#973](https://github.com/IvanWng97/pixtuoid/pull/973))
- *(fixtures)* placeholder the operator's installed skill and agent roster ([#993](https://github.com/IvanWng97/pixtuoid/pull/993))
- *(capture)* close the three identity classes the recorder's gate could not see ([#992](https://github.com/IvanWng97/pixtuoid/pull/992))

### Documentation

- the latest-CLI-only rule becomes a convention, and three cleanups ([#994](https://github.com/IvanWng97/pixtuoid/pull/994))

### Features

- *(scene)* [**breaking**] the wall board drops its version, flaps its mood line, and glows like a neon sign ([#1009](https://github.com/IvanWng97/pixtuoid/pull/1009))
- *(tui)* the version popup links to the GitHub release instead of shipping notes ([#1006](https://github.com/IvanWng97/pixtuoid/pull/1006))
- *(dsh)* the subagent dispatch mints Task — settled by two authed captures ([#972](https://github.com/IvanWng97/pixtuoid/pull/972))
- *(capture)* blank every subtree no decoder reads — allowlist derived by probe ([#997](https://github.com/IvanWng97/pixtuoid/pull/997))

### Miscellaneous

- *(capture)* drop the capture-time PII key chase for a corpus invariant ([#999](https://github.com/IvanWng97/pixtuoid/pull/999))
- *(fixtures)* strip the committed corpus offline — just restrip-fixtures, with exemptions and a verbatim gate ([#998](https://github.com/IvanWng97/pixtuoid/pull/998))
- *(publish)* keep the recorded fixture corpus out of the crates.io tarball ([#996](https://github.com/IvanWng97/pixtuoid/pull/996))

### Performance

- *(site)* the home page's TTI halves — a size-tuned wasm, deferred proof posters, and a Lighthouse that prices gzip ([#1010](https://github.com/IvanWng97/pixtuoid/pull/1010))

### Refactoring

- *(log)* every tracing value rides a field — constant messages, wire values on ? ([#977](https://github.com/IvanWng97/pixtuoid/pull/977))

### Testing

- *(fixtures)* re-record seventeen scenarios at the installed CLIs — kimi 2.0 holds ([#1003](https://github.com/IvanWng97/pixtuoid/pull/1003))
- *(fixtures)* re-record twelve headless scenarios at the installed CLIs ([#989](https://github.com/IvanWng97/pixtuoid/pull/989))
- *(grok)* re-record tool-run at 1.0.25, bump verified_version ([#988](https://github.com/IvanWng97/pixtuoid/pull/988))

### Build

- *(deps)* bump the rust-deps group with 2 updates ([#1002](https://github.com/IvanWng97/pixtuoid/pull/1002))

### Ci

- *(release)* release-plz derives the bump and tags a merged release PR ([#1007](https://github.com/IvanWng97/pixtuoid/pull/1007))
