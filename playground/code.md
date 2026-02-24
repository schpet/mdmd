# Code Blocks

← [Back to index](README.md)

## Rust

```rust
use std::collections::HashMap;

fn word_count(text: &str) -> HashMap<&str, usize> {
    let mut counts = HashMap::new();
    for word in text.split_whitespace() {
        *counts.entry(word).or_insert(0) += 1;
    }
    counts
}

fn main() {
    let text = "hello world hello rust world";
    let counts = word_count(text);
    for (word, count) in &counts {
        println!("{word}: {count}");
    }
}
```

## TypeScript

```typescript
interface User {
  id: number;
  name: string;
  email: string;
}

async function fetchUser(id: number): Promise<User> {
  const res = await fetch(`/api/users/${id}`);
  if (!res.ok) throw new Error(`HTTP ${res.status}`);
  return res.json() as Promise<User>;
}
```

## Python

```python
from dataclasses import dataclass
from typing import Iterator

@dataclass
class Range:
    start: int
    stop: int
    step: int = 1

    def __iter__(self) -> Iterator[int]:
        current = self.start
        while current < self.stop:
            yield current
            current += self.step

for n in Range(0, 10, 2):
    print(n)
```

## Shell

```bash
#!/usr/bin/env bash
set -euo pipefail

PORT=${PORT:-3333}
FILE=${1:?usage: serve.sh <file>}

cargo run -- serve "$FILE" --port "$PORT"
```

## TOML

```toml
[package]
name = "mdmd"
version = "0.1.0"
edition = "2021"

[dependencies]
axum = "0.7"
tokio = { version = "1", features = ["full"] }
comrak = { version = "0.21", features = ["default"] }
```

## JSON

```json
{
  "name": "schpet-oss",
  "tailscale": {
    "Self": {
      "DNSName": "schpet-oss.tail575c.ts.net."
    }
  }
}
```

## Inline Code

Use `cargo build --release` to build. The entry point is `src/main.rs`. The `serve_root` is derived from the directory containing the initial file.
