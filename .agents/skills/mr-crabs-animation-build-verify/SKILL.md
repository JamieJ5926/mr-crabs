---
name: mr-crabs-animation-build-verify
description: "Build and verify Mr Crabs identity, cursor-trail, and streaming-text animation changes in the Ghostty fork."
---

# Mr Crabs animation release gate

Repository: `/Users/jamie/Documents/Projects/active/mr-crabs`.

## Invariants

- Product bundle: `Mr Crabs.app`
- Bundle ID: `dev.jamie.mr-crabs`
- Internal executable remains `ghostty`; do not rename it casually.
- Preserve Ghostty features and existing dirty animation work.

## Toolchain

Use:

```bash
/Users/jamie/.local/opt/zig-aarch64-macos-0.16.0/zig
```

## Focused checks

```bash
zig build test -Dtest-filter="text animation"
zig build test -Dtest-filter="cursor trail"
zig build test -Dtest-filter="pager:"
```

Then build the actual standalone app:

```bash
zig build -Doptimize=ReleaseFast
```

Expected artifact: `zig-out/Mr Crabs.app`.

## Identity and signature checks

Read `CFBundleIdentifier`, `CFBundleName`, and `CFBundleDisplayName` from `zig-out/Mr Crabs.app/Contents/Info.plist`; expect `dev.jamie.mr-crabs` and `Mr Crabs`. Run deep strict `codesign --verify` against the bundle.

Verify runtime defaults using:

```bash
'./zig-out/Mr Crabs.app/Contents/MacOS/ghostty' +show-config --default --docs
```

Relevant expected defaults after the initial animation cutover:

```ini
cursor-trail = true
text-animation = streaming
text-animation-duration = 120ms
text-animation-intensity = 1
```

## Behavioral smoke

Launch the actual bundled executable with an isolated config and a shell script that emits text incrementally. Capture the Mr Crabs window, inspect branding/text integrity/reveal bloom, then stop only the smoke process. A static screenshot proves visual state, not temporal animation; use multiple frames or a frame sequence when motion itself must be proven.
