# Installing

{{#include ../../../README.md:install}}

In CI and in a team, install one version everywhere. Two versions of the generator write different code and a different `honig` dependency:

```sh
cargo binstall cargo-honig@x.y.z
```

## Updating

{{#include ../../../README.md:upgrade}}

A bot such as Renovate or Dependabot bumps `honig` alone, without the generator, so the generated code no longer matches. Turn it off for `honig`. In `renovate.json`:

```json
{
  "packageRules": [{ "matchPackageNames": ["honig"], "enabled": false }]
}
```
