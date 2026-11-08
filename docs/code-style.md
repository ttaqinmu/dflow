# Rust code style

Run `cargo fmt --all` before committing and `cargo fmt --all -- --check` when checking a change. The root [`rustfmt.toml`](../rustfmt.toml) sets the stable rustfmt style edition and line width for this crate.

Keep related code easy to scan:

- Put **one blank line** between top-level items: types, constants, statics, free functions, traits, implementations, and modules. Apply the same rule between methods and associated items inside an `impl` or `trait`.
- Keep doc comments and attributes directly attached to the item they describe. Put the separating blank line **before** the comment or attribute, not between it and the declaration.
- Group imports by purpose and leave one blank line after the final import group before the first declaration.
- Use a blank line inside a function to separate distinct steps. Avoid blank lines after an opening brace or before a closing brace, and avoid splitting a short, cohesive sequence just to add space.
- Prefer named local variables or small helper functions when a dense expression obscures its purpose. Do not force a one-line expression to be multiline solely to meet a line count.

For example:

```rust
use std::path::Path;

/// A source file selected for analysis.
#[derive(Debug)]
struct SourceFile {
    path: String,
}

impl SourceFile {
    fn new(path: &Path) -> Self {
        Self {
            path: path.display().to_string(),
        }
    }

    fn name(&self) -> &str {
        &self.path
    }
}

fn analyze(file: &SourceFile) {
    let name = file.name();

    println!("{name}");
}
```

`cargo fmt` handles indentation, wrapping, and other supported layout rules. It generally preserves existing blank lines, but **stable rustfmt cannot require a blank line between items**. Consequently, `cargo fmt --check` cannot enforce the first two spacing rules; review those when changing nearby code. Apply these rules to new and edited code without reformatting unrelated files solely for spacing.
