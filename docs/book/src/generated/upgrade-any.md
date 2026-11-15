<!-- Generated from src/upgrade.rs by src/tests/docs_pages.rs. Edit the source, not this file. -->

In any version:

- A field that must be a boolean, such as `required`, may hold a YAML 1.1 word: `yes`, `no`, `on`, `off`, `y` or
  `n`. honig reads it as `true` or `false`.
- An unknown `type` in a schema is ignored.
- A `discriminator` whose property is optional and that has no `defaultMapping` gets a warning. A value without the
  property decodes into an untyped variant.
