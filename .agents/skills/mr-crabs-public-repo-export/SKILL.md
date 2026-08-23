---
name: mr-crabs-public-repo-export
description: Export harden commits from mr-crabs-rust into the clean public JamieJ5926/mr-crabs repo without the Ghostty history
---

# Public Mr Crabs export

Use when landing standalone Mr Crabs work from `/Users/jamie/Documents/Projects/active/mr-crabs-rust` onto https://github.com/JamieJ5926/mr-crabs.

## Trees

- Dev: `/Users/jamie/Documents/Projects/active/mr-crabs-rust` branch `rust-rewrite` — 17k Ghostty commits. Remotes include `migration/main`. **Never** `git push` this origin/main as the public product.
- Public checkout: `/Users/jamie/Documents/Projects/active/mr-crabs-terminal` — shallow clone of `JamieJ5926/mr-crabs`, clean 3-commit history (`e3cb442` → `a007023` → later harden).

If the public checkout is missing:

```bash
git clone --depth 5 git@github.com:JamieJ5926/mr-crabs.git /Users/jamie/Documents/Projects/active/mr-crabs-terminal
```

## Copy

Copy only the files that changed. Typical harden slice:

```bash
cp crates/mr-crabs-terminal/src/storage.rs \
  /Users/jamie/Documents/Projects/active/mr-crabs-terminal/crates/mr-crabs-terminal/src/storage.rs
```

Do not rsync the whole rust worktree (would drag Ghostty leftover paths and dirty LICENSE/README from the fork).

## Push

Commit and push **only** from `mr-crabs-terminal`:

```bash
git -C /Users/jamie/Documents/Projects/active/mr-crabs-terminal push origin main
```

Verify `gh repo view JamieJ5926/mr-crabs --json isFork,isPrivate` → `isFork=false`, `isPrivate=false`.

## Related contracts

- Scrollback: queue Full keeps pages hot; no feed-thread LZ4. See `storage.rs` header overload behavior.
- Config docs live in the public `README.md` (`--config-file` JSON, `Cmd+Shift+R`).
