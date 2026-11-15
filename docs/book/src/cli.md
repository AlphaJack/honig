# Command line

The command line has six commands. `cargo honig --help` lists them:

```text
{{#include generated/cli-help.txt}}
```

## Check a description

`validate` reads a description under its own version and prints what is wrong. It exits with 1 on an error.

```sh
$ cargo honig validate openapi.yaml
ok: openapi.yaml
$ cargo honig validate broken.yaml
error: /info: missing field `version`
```

Each diagnostic starts with a JSON pointer into the document.

## See what honig reads

`upgrade` prints the document as honig generates from it: OpenAPI 3.2 JSON.
Run it to see what a Swagger 2.0 or 3.0 description becomes before you generate.

```sh
cargo honig upgrade swagger.yaml > openapi.json
```

[Upgrading](upgrading.md) lists what changes on the way.

## Merge a description split over files

`bundle` prints one document. Each `$ref` to another file becomes a component, named after the file.
The version stays the one the description is written in.

```sh
cargo honig bundle openapi.yaml > bundled.json
```

A `$ref: order.yaml` becomes `$ref: '#/components/schemas/order'`. Component names and `$ref`s stay, so other tools such as TypeScript generators and API reference pages read the same document. `--format yaml` prints it as YAML.
