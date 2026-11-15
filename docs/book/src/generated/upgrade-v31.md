<!-- Generated from src/upgrade/v31_to_v32.rs by src/tests/docs_pages.rs. Edit the source, not this file. -->

OpenAPI 3.1 to 3.2.

| 3.1 | Becomes |
| --- | --- |
| array-form `items` (JSON Schema 2019-09 and older) | `prefixItems` |
| an object where an array is required | an array of its values |
| `xml.attribute` and `xml.wrapped` | `xml.nodeType`. With both, `attribute` wins |
| `x-tagGroups` | `tags[].parent`, only when the mapping is unambiguous |
| registered `x-oai-*` extensions | the 3.2 fields they carry: `$self`, `additionalOperations`, `itemSchema`, `itemEncoding`, `prefixEncoding`, `encoding`, `dataValue`, `serializedValue`, the `name` of a server, the `summary` of a response and the `deprecated` and device authorization flow of a security scheme |

What is lost:

- `allowReserved` on a parameter that is not in the query. It was inactive in 3.1 and would change meaning in 3.2.
