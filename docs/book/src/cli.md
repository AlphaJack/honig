# Command line

`cargo honig --help` lists the commands:

```text
{{#include generated/cli-help.txt}}
```

## Check a description

`validate` checks a description against its own version and prints what is wrong. It exits with 1 on an error.

```sh
$ cargo honig validate openapi.yaml
ok: openapi.yaml
$ cargo honig validate broken.yaml
error: /info: missing field `version`
```

Each diagnostic starts with a JSON pointer into the document. `generate` fails on the same errors: code from a description with an error would differ from what it says. Warnings do not stop it. With `--deny-warnings`, `validate` and `generate` fail on a warning as on an error, so CI can refuse one.

## A description from someone else

honig follows every `$ref` and `externalValue` of a description. A file ref may name any path on your machine, also outside the folder of the description, such as `../../secrets.yaml` or an absolute path. A URL ref is fetched, also from a host of your internal network. So a description that you did not write can make `generate` and `validate` read your files and call your hosts.

Run honig only on descriptions you trust. To fetch no URL at all, install the generator without remote refs:

```sh
cargo install cargo-honig --no-default-features --features overlays
```

A URL ref then fails generation with a pointer to it.
