<!-- Generated from runtime/src/reject.rs by src/tests/docs_pages.rs. Edit the source, not this file. -->

| Variant | Meaning |
|---|---|
| `Status` | The HTTP status as a number (`code` or `status`). |
| `StatusText` | The HTTP status as text (`code` declared as a string). |
| `Title` | The reason phrase (`title`). |
| `Detail` | What went wrong (`message`, `detail` or `error`). |
| `Type` | `about:blank` (`type`). |
| `Violations` | Every violation as `{path, message}` (`errors` or `violations`). |
| `ViolationMessages` | Every violation as one string each (`errors` of strings). |
| `Const` | A required `const` (`object: {const: error}`): its one value, as JSON text. |
| `Empty` | A required field nothing maps to: its type's empty value. |
