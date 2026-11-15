<!-- Generated from src/upgrade/v20_to_v30.rs by src/tests/docs_pages.rs. Edit the source, not this file. -->

Swagger 2.0 to OpenAPI 3.0.

| 2.0 | Becomes |
| --- | --- |
| `host`, `basePath`, `schemes` | `servers` |
| `definitions`, `parameters`, `responses`, `securityDefinitions` | `components`, with every `$ref` rewritten |
| `body` and `formData` parameters | `requestBody`, one entry per `consumes` media type |
| a response `schema` | `content`, one entry per `produces` media type |
| inline type keywords of other parameters and headers | their `schema` |
| `collectionFormat` | `style` and `explode` |
| `collectionFormat: tsv` | `x-collection-format: tsv`: 3.x has no tab style, but honig still encodes it |
| `type: file` | `type: string` with `format: binary` |
| `x-nullable` | `nullable` |

What is lost:

- An unknown `collectionFormat` is read as `csv`.
- An unknown `OAuth2` `flow` is read as `implicit`.
- A security scheme of an unknown `type` is kept as written. Generated code does not use it.
