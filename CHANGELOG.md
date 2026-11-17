# Changelog

## [0.0.2](https://github.com/AlphaJack/honig/compare/v0.0.1...v0.0.2) - 2026-11-17

### Added

- **cli**: Add mod_rs to write the root of a module beside its folder ([#40](https://github.com/AlphaJack/honig/pull/40))
- **runtime**: Export tower's Service trait ([#36](https://github.com/AlphaJack/honig/pull/36))

### Fixed

- **generator**: Validate the text of a library type in a request parameter or body ([#51](https://github.com/AlphaJack/honig/pull/51))
- **generator**: Validate the text of a library type in a union branch or a newtype of a request ([#44](https://github.com/AlphaJack/honig/pull/44))
- **generator**: Validate the text of a library type in a field before the library reads it ([#43](https://github.com/AlphaJack/honig/pull/43))
- **generator**: Resolve a pointer only through RFC 6901 array indexes ([#39](https://github.com/AlphaJack/honig/pull/39))
- **generator**: Keep the offset of format: time in runtime::FullTime ([#38](https://github.com/AlphaJack/honig/pull/38))
- **generator**: Leave a body of your own type to that type ([#34](https://github.com/AlphaJack/honig/pull/34))
- **generator**: Keep each file's servers on a shared path ([#25](https://github.com/AlphaJack/honig/pull/25))
- **runtime**: Validate the text of a library type in each item of a streamed request ([#53](https://github.com/AlphaJack/honig/pull/53))
- **runtime**: Validate the address of a CIDR block as ipv4 and ipv6 do ([#42](https://github.com/AlphaJack/honig/pull/42))
- **runtime**: Decode JSON as deep in debug builds as in release ([#37](https://github.com/AlphaJack/honig/pull/37))
- **runtime**: Name the field of the closest branch when no union branch matches ([#31](https://github.com/AlphaJack/honig/pull/31))

### Performance

- **runtime**: Look up only the keywords a schema object holds ([#33](https://github.com/AlphaJack/honig/pull/33))

## [0.0.1](https://github.com/AlphaJack/honig/releases/tag/v0.0.1) - 2026-11-16

### Added

- Publish honig

### Fixed

- Join refs with / on Windows and repair CI ([#1](https://github.com/AlphaJack/honig/pull/1))
