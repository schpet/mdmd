---
title: Frontmatter Demo
author: mdmd contributors
status: draft
version: 0.1.0
tags:
  - yaml
  - frontmatter
  - demo
settings:
  toc: true
  theme: default
---

# Frontmatter Demo

← [Back to index](README.md)

This page demonstrates YAML frontmatter support. The metadata above is extracted and displayed in a panel before the document body.

## What is Frontmatter?

Frontmatter is a block of YAML metadata at the very top of a Markdown file, delimited by `---` fences. It lets you attach structured key-value data to a document without it appearing in the rendered body.

## Supported Value Types

The frontmatter parser handles:

- **Scalars** — strings, numbers, booleans (`author`, `status`, `version`)
- **Sequences** — lists of values (`tags`)
- **Mappings** — nested key-value pairs (`settings`)
- **Null** — explicitly empty values

## Example

A minimal frontmatter block:

```yaml
---
title: My Document
---
```

A richer example with nesting:

```yaml
---
title: Project Plan
author: Jane Doe
priority: high
tags:
  - planning
  - q2
config:
  draft: true
  reviewers: 3
---
```

## Notes

- The `title` field, when present, is used as the page title.
- Frontmatter is stripped from the rendered Markdown body.
- Malformed YAML gracefully falls back to rendering the raw source.
