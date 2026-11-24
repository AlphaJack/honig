# Types

## Request and response types

A schema with `readOnly` or `writeOnly` properties becomes two types:

| Type | Used for | Fields |
|---|---|---|
| `Login` | responses | `token` (`readOnly`). No `password`. |
| `LoginRequest` | request bodies and parameters | `password` (`writeOnly`). No `token`. |

Each is required in Rust where the schema requires it. A missing password is a compile error and not a runtime check. Types that contain one split too. An example is `Session` and `SessionRequest`.

Every struct has a builder that accepts each property by name: `build` exists once every required one is set, so leaving one out is a compile error, and the order of the properties in the description does not matter. A struct also has `new` when that cannot go wrong: no required property is of a checked type and no two have the same type, so a swap is a type error. `new` accepts the required properties; a property with a `default` starts at it, and any other optional property starts empty:

```rust
{{#include ../../../tests/e2e/tests/v32_catalog/schemas.rs:new}}
```

A property added to the description as required becomes an argument. Code that builds the type stops compiling until it sets it.

A property that is optional and nullable is `api::Nullable<T>`. Its variants are `Absent`, `Null` and `Value(T)`. This is how a `PATCH` body tells "leave it" from "clear it". A property that is only one of required or nullable is `Option<T>`.

A `const` is a unit struct that only decodes its value. Arrays with `minItems == maxItems` are `[T; N]`. The formats `int8` to `uint16` are the Rust integers of the same width.

## Enums and unions

String and integer enums are closed. Each listed value is a variant. Any other value fails `try_from` and decoding. So a request with one gets `400`, and a client decoding a response with one gets a decode error. Update the description and generate again when the server adds a value.

A `oneOf` or `anyOf` is closed too: a value that no branch accepts fails decoding. An `anyOf` is the first branch that decodes, keywords and all. So two object branches of an `anyOf`, or of a `oneOf` without a discriminator, that one value can match fail generation: the first would keep the value and drop what the other declares. honig is stricter than JSON Schema here. A required property that the other branch lacks or types otherwise, a `const` tag, or `additionalProperties: false` on one tells them apart. A `oneOf` decodes every branch and accepts the value only when exactly one does. A union has an `Other(serde_json::Value)` variant only when the schema itself accepts any value. That is a branch of `{}`, or keywords such as `properties` without a `type`. Then any value of another kind is valid.

## Values the description does not list

Two settings decide what happens to a property or a value that the description does not list. They answer different questions:

| Setting | Applies to | What it does |
|---|---|---|
| `keep_unknown_fields = true` | objects without `additionalProperties`, both directions | keeps the undeclared properties in a map, so a value round-trips. The default `false` ignores them |
| `client = { allow_unknown_values = true }` | the responses of a client | accepts what a server added: an enum value or a union branch as `Other`, and a property of a closed object (`additionalProperties: false`) by ignoring it. Requests stay strict through `<Name>Request` copies |

An object without `additionalProperties` never refuses an undeclared property: the first setting only chooses whether to keep it. A closed object refuses it, unless `allow_unknown_values = true` applies to the response.

## Conditional schemas

`if`/`then` blocks that test one property against constants become an enum. It is like a discriminated `oneOf`:

```yaml
Notify:
  type: object
  properties: {type: {type: string}}
  allOf:
    - {if: {properties: {type: {const: email}}}, then: {required: [email], properties: {email: {type: string}}}}
    - {if: {properties: {type: {const: sms}}},   then: {required: [phone], properties: {phone: {type: string}}}}
```

This gives `enum Notify { Email(NotifyEmail), Sms(NotifySms), Otherwise(NotifyOtherwise) }`. `NotifyEmail` has `email: String` as required.

- `Otherwise` contains every other value. Its type has the base properties.
- A lone `if`/`then`/`else` becomes the same kind of enum. `else` is the `Otherwise` variant. `else: false` leaves none.
- An `if` on `enum: [email, mail]` gives one variant for both values: `EmailOrMail`.
- Conditions that cannot be typed, such as ranges or `not`, are still enforced. The server checks them on every request.

`$dynamicRef` generics are monomorphized. `ProductPage: {$ref: Page, $defs: {item: {$dynamicAnchor: item, $ref: Product}}}` becomes a struct with `items: Vec<Product>`. A recursive `Tree<T>` bound to `string` becomes `StringTree { value: String, children: Vec<StringTree> }`.

## Your own derives and types

Generated types accept extra derives. Use `"*"` for all types or the Rust name of one type:

```toml
[package.metadata.honig.api.derives]
"*" = ["Hash"]
Order = ["sqlx::FromRow"]
```

Derives the generator already emits are skipped. String and integer enums are closed and `Copy`: one unit variant per listed value. So derives that need unit-only enums fit them, such as `sqlx::Type`.

Put `x-rust-type` on the schema to reuse a type that you already have. honig then generates no type for it:

```yaml
Money:
  x-rust-type: crate::domain::Money
  type: object
  ...
```

`types::Money` becomes an alias of your type. It must serialize like the schema. It must implement `Debug`, `Clone`, `PartialEq` and the serde traits. It must also check the constraints of the schema: honig checks none of them, neither where the schema is declared nor where a body or a field uses it.

A type from another crate uses the object form:

```yaml
OrderId:
  type: string
  format: uuid
  x-rust-type: {crate: order-ids, version: "*", path: order_ids::OrderId}
```

honig uses it only when your crate depends on that crate. Otherwise it warns and generates the type.

## String formats

The type features of the `honig` dependency pick the type of each string format:

```toml
[dependencies]
honig = { version = "=x.y.z", features = ["client", "server", "uuid", "url", "ipnet", "jiff"] }
```

The runtime re-exports each crate. So generated code refers to `api::runtime::uuid::Uuid` and your crate needs no `uuid` dependency of its own. Your code can still write `uuid::Uuid` with its own `uuid` dependency of the same major version: cargo builds one copy.

{{#include generated/formats.md}}

The `formats` table gives a format with a check a type of your own:

```toml
[package.metadata.honig.api]
formats = { date-time = "crate::Stamp", uuid = "crate::ids::OrderId" }
```

The key is the name of the format. The value is the path of a type, without generic arguments. It wins over a feature for its format only: with `jiff` on, `date` above stays `jiff::civil::Date`. The type must implement `Debug`, `Clone`, `PartialEq`, `Eq` and the serde traits. It parses text that honig checked first, as a feature's type does. honig knows the quirks of its own feature types only. For any type of `date-time`, `date-time-local` and `time-local` it cuts a fraction of a second to nine digits. honig does not check the type: the compiler reports a wrong one. A struct with a required field of a type of yours derives no `Default`.

`format: time` has an offset (`08:30:06+02:00`), and no `jiff` time type supports one. So it is `api::FullTime` with or without `jiff`: plain numbers (`hour`, `minute`, `second`, `nanosecond`, `offset_seconds`) and whether the second was a leap second. With `jiff` on, `time()` and `offset()` return it as `jiff` types.

A format without its feature is its checked text, such as `api::UuidStr` or `api::DateTimeStr`. Only text of the format is one: `try_from`, `parse` and decoding check it as the library would on parsing. It derefs to `str`. Some checks need a runtime feature. `generate` stops and prints the `cargo add` line when it is off. [Adapters](adapters.md) lists the features.

A library accepts more than its format allows. `jiff` accepts `2024-06-19 08:30Z` as a `date-time`. `uuid` accepts `{...}` and `url` accepts a space. So a library type in a property, a branch of a union, a newtype, a parameter or a body has its text validated first, as a `String` of that format is. The items of a body decoded as it arrives are validated one at a time. Then the library parses it. A server decoding a request and a client decoding a response check alike. A fraction of a second past nine digits is cut: `jiff` stores nanoseconds.

Mail addresses and host names are types of the runtime. Only a valid value can be one of them. `try_from` and decoding check the value. They need the `idn` feature of the runtime when the description uses them:

```rust
{{#include ../../../tests/e2e/tests/v32_catalog/schemas.rs:email}}
```

{{#include generated/checked-text.md}}

`http-date` is `api::HttpDateStr`, one of the three forms of RFC 9110 section 5.6.7. `duration` is `api::DurationStr`, an ISO 8601 duration of RFC 3339 Appendix A. No std type represents `P1M`: a month has no fixed length. `uri-reference`, `iri`, `iri-reference`, `json-pointer`, `relative-json-pointer`, `uri-template` and `regex` are checked text of their grammars (RFC 3986, 3987, 6901, 6570 and ECMA-262). `uuid` without its feature accepts only the hyphenated form of RFC 9562.

`contentEncoding: base64url` gives `api::Base64Url`. It stores bytes in the URL-safe alphabet.

`{type: number, format: decimal}` and `decimal128` are `api::Decimal`. It keeps the digits of the number. `12345678901234567.89` arrives and leaves as written. An `f64` would round it.
It parses the exact text wherever the number is. This includes `oneOf` branches. It rounds only in the `Other` of an open union. That variant stores a value that no branch accepts as a `serde_json::Value`.

For arithmetic parse its digits into a decimal type with `d.as_str().parse::<rust_decimal::Decimal>()`.
`{type: string, format: decimal}` is `api::DecimalStr` with every digit, checked as a decimal. `decimal128` keeps every digit too, as a number or as `api::Decimal128Str`, and enforces the limits of IEEE 754 decimal128: at most 34 significant digits within its exponent range. Use it past the 28 significant digits that `rust_decimal` supports.

JSON the description leaves untyped is a `serde_json::Value`:

- the properties that an object does not declare with `keep_unknown_fields = true`
- a `{}` schema and an untyped `additionalProperties`
- the `Other` of an open union

A number in such a value rounds when it is past `u64`. It also rounds when it has more significant digits than an `f64` stores, which is about 17. RFC 8259 section 6 allows this. A client that decodes such an object and writes it back can change that number. To keep a number exact declare it with `format: decimal`.

## Checked types

A string with a `pattern`, `minLength` or `maxLength` is a type of its own. So is an integer or number with `minimum`, `maximum`, `exclusiveMinimum`, `exclusiveMaximum` or `multipleOf`. Only valid values can be that type: `try_from` and decoding check the keywords and return the first one that the value breaks. The `regress` feature of the runtime checks `pattern`. The generated code needs it when the description uses `pattern`. A `pattern` beside a `format` that has a type of its own, such as `uuid` or `date-time`, also fails generation: honig is stricter than JSON Schema here, because that type decodes the value and no check would run the pattern. A `pattern` that is not an ECMA-262 regular expression fails generation: its type could not check it:

```rust
{{#include ../../../tests/e2e/tests/v32_catalog/schemas.rs:pattern}}
```

So a request with a bad value gets `400`, and a client decoding a response with one gets a decode error.

Where the field type is known, `try_into` builds the value. A spec `default` is filled in already checked:

```rust
{{#include ../../../tests/e2e/tests/v32_catalog/schemas.rs:try-into}}
```

A builder converts every field and returns the first one that does not convert, from `build`. So the whole value needs one `?`. Each setter accepts anything that converts to the type of its field: a number for a checked number, a `&str` for a string, a checked string or an enum:

```rust
{{#include ../../../tests/e2e/tests/v32_validation.rs:builder}}
```

The error points to the field with a JSON pointer, as a refused request does: `/age: 150 must be less than 150`. The builder of `User` is `types::builders::User`. Where `new` exists, it and struct update build the value with no `?`.

`try_from`, `parse` and `build` all fail with `api::BuildError`, so a function that mixes them needs no `Box<dyn Error>`. In a handler, `?` turns it into a `500`: the handler's inputs are checked already, so a value that its schema refuses came from the server's own data.

A checked type behaves as its value: a string one derefs to `str` and compares with `&str`, a number one derefs to its number and compares with it. `get` and `into_inner` return the value, and `parse` parses one from text.

Integer keywords are exact. A fractional bound rounds inward, `multipleOf: 2.5` accepts the integers that are multiples of 5, and a bound that no value of the type meets refuses every value: `maximum: -1` on a `uint8` leaves none.

## Whole schemas

Some keywords say what no Rust type can: `not`, `contains`, `dependentRequired`, `if`/`then` beyond the constants of one property, the one branch that a `oneOf` accepts when a discriminator picks branches it does not pin, tuples and `$dynamicRef`. A struct or union whose own schema has them is a checked type of its shape: `Product` wraps a `ProductFields`, `Vehicle` wraps a `VehicleVariants`. It derefs to its shape. `try_from` and decoding check the whole schema. Its builder fills the fields and checks the whole value in `build`:

```rust
{{#include ../../../tests/e2e/tests/v32_kitchen_sink.rs:whole-schema}}
```

A property, parameter or body with such keywords of its own has a checked type named after it, such as `ProductAttrs`. A type of yours (`x-rust-type`) is never wrapped: it keeps its own invariants.
