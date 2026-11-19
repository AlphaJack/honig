# Changelog

## [0.0.3](https://github.com/AlphaJack/honig/compare/v0.0.2...v0.0.3) - 2026-11-18

### Added

#### generator

- Warn that a pattern beside a typed format is not checked ([#78](https://github.com/AlphaJack/honig/pull/78))

### Fixed

#### generator

- Find the kind and size of each type once ([#84](https://github.com/AlphaJack/honig/pull/84))
- Hold decimal128 to the limits of IEEE 754 decimal128 ([#82](https://github.com/AlphaJack/honig/pull/82))
- Put each schema that a whole check reaches in $defs once ([#81](https://github.com/AlphaJack/honig/pull/81))
- Print the body of a generated macro as items ([#80](https://github.com/AlphaJack/honig/pull/80))
- Check integer bounds and multipleOf exactly ([#76](https://github.com/AlphaJack/honig/pull/76))
- Check a cycle past the depth limit and warn where a whole schema stops ([#75](https://github.com/AlphaJack/honig/pull/75))
- Key a cycle of a schema checked whole by its pointer ([#74](https://github.com/AlphaJack/honig/pull/74))
- Rewrite the $ref of a property named default, enum or example ([#73](https://github.com/AlphaJack/honig/pull/73))
- Let a binary that denies allow_attributes take the generated code ([#71](https://github.com/AlphaJack/honig/pull/71))

#### runtime

- Check decimal ranges and multipleOf exactly on decimal text ([#77](https://github.com/AlphaJack/honig/pull/77))
- Name the field of a form body that does not decode ([#79](https://github.com/AlphaJack/honig/pull/79))

## [0.0.2](https://github.com/AlphaJack/honig/compare/v0.0.1...v0.0.2) - 2026-11-17

### Added

#### cli

- Add mod_rs to write the root of a module beside its folder ([#40](https://github.com/AlphaJack/honig/pull/40))

#### runtime

- Export tower's Service trait ([#36](https://github.com/AlphaJack/honig/pull/36))

### Fixed

#### generator

- Validate the text of a library type in a request parameter or body ([#51](https://github.com/AlphaJack/honig/pull/51))
- Validate the text of a library type in a union branch or a newtype of a request ([#44](https://github.com/AlphaJack/honig/pull/44))
- Validate the text of a library type in a field before the library reads it ([#43](https://github.com/AlphaJack/honig/pull/43))
- Resolve a pointer only through RFC 6901 array indexes ([#39](https://github.com/AlphaJack/honig/pull/39))
- Keep the offset of format: time in runtime::FullTime ([#38](https://github.com/AlphaJack/honig/pull/38))
- Leave a body of your own type to that type ([#34](https://github.com/AlphaJack/honig/pull/34))
- Keep each file's servers on a shared path ([#25](https://github.com/AlphaJack/honig/pull/25))

#### runtime

- Validate the text of a library type in each item of a streamed request ([#53](https://github.com/AlphaJack/honig/pull/53))
- Validate the address of a CIDR block as ipv4 and ipv6 do ([#42](https://github.com/AlphaJack/honig/pull/42))
- Decode JSON as deep in debug builds as in release ([#37](https://github.com/AlphaJack/honig/pull/37))
- Name the field of the closest branch when no union branch matches ([#31](https://github.com/AlphaJack/honig/pull/31))

### Optimized

#### runtime

- Look up only the keywords a schema object holds ([#33](https://github.com/AlphaJack/honig/pull/33))

## [0.0.1](https://github.com/AlphaJack/honig/releases/tag/v0.0.1) - 2026-11-16

### Added

- Publish honig

### Fixed

- Join refs with / on Windows and repair CI ([#1](https://github.com/AlphaJack/honig/pull/1))
