# Upgrading

honig generates from OpenAPI 3.2 only. A 2.0, 3.0 or 3.1 description is upgraded one version at a time: 2.0 to 3.0, 3.0 to 3.1, then 3.1 to 3.2.
Run `cargo honig upgrade` to see the result.

Most of a description carries over unchanged. Each change below prints a warning that points at the field.
`cargo honig generate` prints them unless you pass `--quiet`.

## Swagger 2.0

{{#include generated/upgrade-v20.md}}

## OpenAPI 3.0

{{#include generated/upgrade-v30.md}}

## OpenAPI 3.1

{{#include generated/upgrade-v31.md}}
