

<!-- release-instructions-v1 -->

---

## Making a Release

Releases are managed with `just release` (see `justfile`).

### Prerequisites

- [`changelog`](https://github.com/your-org/changelog-cli) CLI must be installed
- [`svbump`](https://github.com/your-org/svbump) CLI must be installed
- Must be on the `main` branch with a clean working copy

### Steps

1. **Add changelog entries** throughout development using the CLI (never edit `CHANGELOG.md` by hand):

   ```bash
   changelog add --type added "description of new feature"
   changelog add --type fixed "description of bug fix"
   changelog add --type changed "description of behavior change"
   ```

2. **Cut the release** with a single command:

   ```bash
   just release patch   # 1.0.0 → 1.0.1
   just release minor   # 1.0.0 → 1.1.0
   just release major   # 1.0.0 → 2.0.0
   just release 1.2.3   # explicit version
   ```

   This command automatically:
   - Moves `[Unreleased]` entries in `CHANGELOG.md` to the new version
   - Updates the `package.version` field in `Cargo.toml` via `svbump`
   - Creates a release commit and sets the `main` bookmark
   - Tags the commit as `v<version>`
   - Pushes `main` and the tag to the remote

### Changelog Entry Style

- Use present tense for new behavior: `"X now supports Y"`
- Use past tense for fixes/removals: `"Fixed X"`, `"X has been removed"`
- Descriptions should be lowercase (except proper nouns)
- Types: `added`, `changed`, `deprecated`, `removed`, `fixed`, `security`

<!-- end-release-instructions -->

---

<!-- br-agent-instructions-v1 -->

---

## Beads Workflow Integration

This project uses [beads_rust](https://github.com/Dicklesworthstone/beads_rust) (`br`/`bd`) for issue tracking. Issues are stored in `.beads/` and tracked in git.

### Essential Commands

```bash
# View ready issues (unblocked, not deferred)
br ready              # or: bd ready

# List and search
br list --status=open # All open issues
br show <id>          # Full issue details with dependencies
br search "keyword"   # Full-text search

# Create and update
br create --title="..." --description="..." --type=task --priority=2
br update <id> --status=in_progress
br close <id> --reason="Completed"
br close <id1> <id2>  # Close multiple issues at once

# Sync with git
br sync --flush-only  # Export DB to JSONL
br sync --status      # Check sync status
```

### Workflow Pattern

1. **Start**: Run `br ready` to find actionable work
2. **Claim**: Use `br update <id> --status=in_progress`
3. **Work**: Implement the task
4. **Complete**: Use `br close <id>`
5. **Sync**: Always run `br sync --flush-only` at session end

### Key Concepts

- **Dependencies**: Issues can block other issues. `br ready` shows only unblocked work.
- **Priority**: P0=critical, P1=high, P2=medium, P3=low, P4=backlog (use numbers 0-4, not words)
- **Types**: task, bug, feature, epic, chore, docs, question
- **Blocking**: `br dep add <issue> <depends-on>` to add dependencies

### Session Protocol

**Before ending any session, run this checklist:**

```bash
git status              # Check what changed
git add <files>         # Stage code changes
br sync --flush-only    # Export beads changes to JSONL
git commit -m "..."     # Commit everything
git push                # Push to remote
```

### Best Practices

- Check `br ready` at session start to find available work
- Update status as you work (in_progress → closed)
- Create new issues with `br create` when you discover tasks
- Use descriptive titles and set appropriate priority/type
- Always sync before ending session

<!-- end-br-agent-instructions -->
