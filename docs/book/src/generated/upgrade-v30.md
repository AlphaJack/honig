<!-- Generated from src/upgrade/v30_to_v31.rs by src/tests/docs_pages.rs. Edit the source, not this file. -->

Schemas move from the 3.0 dialect to JSON Schema 2020-12.

| OpenAPI 3.0 | OpenAPI 3.1 |
| --- | --- |
| `nullable: true` | `"null"` in `type`, in `enum` or in an `anyOf` branch |
| boolean `exclusiveMinimum` and `exclusiveMaximum` | their numeric form |
| `example` in a schema | `examples: [example]` |
| `x-jsonschema-*` extensions | the JSON Schema keywords they carry |
| `x-oai-license-identifier` | the license `identifier` |

What is lost:

- `allowEmptyValue` and `allowReserved` on a header. 3.0 allowed them there without effect. 3.1 has neither.
- A registered extension whose field is also set. The field wins.
