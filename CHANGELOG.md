# Changelog

## [0.0.4](https://github.com/AlphaJack/honig/compare/v0.0.3...v0.0.4) - 2026-11-21

### Added

- **Breaking:** Check each credential with a verify method of its scheme on the Api ([#229](https://github.com/AlphaJack/honig/pull/229))
- **Breaking:** Fill only RFC 9457 error bodies, and map any other in the Api's error_response ([#215](https://github.com/AlphaJack/honig/pull/215))

#### Runtime

- **Breaking:** Require authenticate on a server whose description declares security ([#224](https://github.com/AlphaJack/honig/pull/224))
- **Breaking:** Hold a time with its offset in plain numbers ([#113](https://github.com/AlphaJack/honig/pull/113))
- **Breaking:** Validate the text of a library type in responses too ([#59](https://github.com/AlphaJack/honig/pull/59))
- **Breaking:** Return RFC 9457 problem details as a typed Problem ([#225](https://github.com/AlphaJack/honig/pull/225))
- **Breaking:** Fail to parse a Decimal or a FullTime with BuildError ([#103](https://github.com/AlphaJack/honig/pull/103))
- **Breaking:** Hold the one violation of an invalid request ([#92](https://github.com/AlphaJack/honig/pull/92))
- Send an own JSON error body with Rejection::json ([#232](https://github.com/AlphaJack/honig/pull/232))
- Hold a sequence to minItems, maxItems and uniqueItems as it arrives ([#68](https://github.com/AlphaJack/honig/pull/68))

#### Generator

- **Breaking:** Let the caller name one of several operation servers ([#246](https://github.com/AlphaJack/honig/pull/246))
- **Breaking:** Build every struct and Params by name, and keep new only where it cannot swap ([#240](https://github.com/AlphaJack/honig/pull/240))
- **Breaking:** Serve a list of operations and keep only the types that the listed operations reach ([#236](https://github.com/AlphaJack/honig/pull/236))
- **Breaking:** List the servers once as client::SERVERS ([#233](https://github.com/AlphaJack/honig/pull/233))
- **Breaking:** Fill a required const field in new and builder instead of taking it ([#231](https://github.com/AlphaJack/honig/pull/231))
- **Breaking:** Write a default into the code as the description has it ([#212](https://github.com/AlphaJack/honig/pull/212))
- **Breaking:** Make a list or map component with keywords a checked type ([#209](https://github.com/AlphaJack/honig/pull/209))
- **Breaking:** Type the values of patternProperties on an object without properties ([#208](https://github.com/AlphaJack/honig/pull/208))
- **Breaking:** Type the values of patternProperties beside declared properties ([#202](https://github.com/AlphaJack/honig/pull/202))
- **Breaking:** Type an enum of objects or mixed values as a union of constants ([#201](https://github.com/AlphaJack/honig/pull/201))
- **Breaking:** Type a const of an object or list as a unit struct ([#200](https://github.com/AlphaJack/honig/pull/200))
- **Breaking:** Keep the absence of a property that a dependent keyword reads ([#191](https://github.com/AlphaJack/honig/pull/191))
- **Breaking:** Build only the structs that a builder shortens ([#99](https://github.com/AlphaJack/honig/pull/99))
- **Breaking:** Require the formats table instead of reading the dependencies ([#98](https://github.com/AlphaJack/honig/pull/98))
- **Breaking:** Return BuildError from try_from and parse ([#93](https://github.com/AlphaJack/honig/pull/93))
- **Breaking:** Check every keyword while decoding and drop runtime::Validate ([#61](https://github.com/AlphaJack/honig/pull/61))
- **Breaking:** Give a format without its library a checked text type ([#124](https://github.com/AlphaJack/honig/pull/124))
- **Breaking:** Give every constrained string and number a checked type ([#57](https://github.com/AlphaJack/honig/pull/57))
- **Breaking:** Close enums and unions ([#56](https://github.com/AlphaJack/honig/pull/56))
- Check every documented example, decoded from its JSON text ([#213](https://github.com/AlphaJack/honig/pull/213))
- Check dependentRequired of a name that the struct does not declare ([#192](https://github.com/AlphaJack/honig/pull/192))
- Bind a generic to the outermost resource that declares its dynamicAnchor ([#189](https://github.com/AlphaJack/honig/pull/189))
- Split draft 7 dependencies into dependentRequired and dependentSchemas ([#187](https://github.com/AlphaJack/honig/pull/187))
- Check keywords beside a $ref in generated code ([#186](https://github.com/AlphaJack/honig/pull/186))
- Check keywords of one kind beside an implied shape in generated code ([#185](https://github.com/AlphaJack/honig/pull/185))
- Count the branches of a oneOf or anyOf that no union tells apart in generated code ([#184](https://github.com/AlphaJack/honig/pull/184))
- Refuse any value of a false schema and type implied shapes in generated code ([#182](https://github.com/AlphaJack/honig/pull/182))
- Check keywords of one kind on a schema without type in generated code ([#180](https://github.com/AlphaJack/honig/pull/180))
- Check dependentSchemas of an undeclared name in generated code ([#176](https://github.com/AlphaJack/honig/pull/176))
- Check propertyNames of any schema in generated code ([#175](https://github.com/AlphaJack/honig/pull/175))
- Bind a generic to a site that declares its dynamicAnchor at its root ([#177](https://github.com/AlphaJack/honig/pull/177))
- Type a dynamicRef to an anchor that one resource declares ([#174](https://github.com/AlphaJack/honig/pull/174))
- Check unevaluatedItems in generated code ([#171](https://github.com/AlphaJack/honig/pull/171))
- Check unevaluatedProperties in generated code ([#170](https://github.com/AlphaJack/honig/pull/170))
- Compare enum and const values that no type holds in generated code ([#167](https://github.com/AlphaJack/honig/pull/167))
- Check each member of an allOf that one type cannot hold in generated code ([#172](https://github.com/AlphaJack/honig/pull/172))
- Check keywords beside a union in generated code ([#166](https://github.com/AlphaJack/honig/pull/166))
- Check tuples item by item in generated code ([#165](https://github.com/AlphaJack/honig/pull/165))
- Check additionalProperties beside patternProperties in generated code ([#173](https://github.com/AlphaJack/honig/pull/173))
- Check patternProperties of several patterns in generated code ([#164](https://github.com/AlphaJack/honig/pull/164))
- Check required, uniqueItems and format without a type in generated code ([#163](https://github.com/AlphaJack/honig/pull/163))
- Count the branches of a tagged oneOf in generated code ([#160](https://github.com/AlphaJack/honig/pull/160))
- Type an anyOf with a branch of any value ([#169](https://github.com/AlphaJack/honig/pull/169))
- Check contains in generated code ([#159](https://github.com/AlphaJack/honig/pull/159))
- Check if, then and else in generated code ([#162](https://github.com/AlphaJack/honig/pull/162))
- Check dependentSchemas in generated code ([#157](https://github.com/AlphaJack/honig/pull/157))
- Check not in generated code ([#156](https://github.com/AlphaJack/honig/pull/156))
- Check keywords without a type in generated code ([#158](https://github.com/AlphaJack/honig/pull/158))
- Check patternProperties in generated code ([#154](https://github.com/AlphaJack/honig/pull/154))
- Check propertyNames in generated code ([#150](https://github.com/AlphaJack/honig/pull/150))
- Check dependentRequired in generated code ([#139](https://github.com/AlphaJack/honig/pull/139))
- Name the type of a string format in the formats table ([#127](https://github.com/AlphaJack/honig/pull/127))
- Read a response whose server added an enum value, a property or a union member ([#101](https://github.com/AlphaJack/honig/pull/101))
- Build a struct from raw values with one error ([#58](https://github.com/AlphaJack/honig/pull/58))
- Turn a response into its success with into_ok ([#100](https://github.com/AlphaJack/honig/pull/100))

#### CLI

- **Breaking:** List the operations of a module once, for both halves ([#262](https://github.com/AlphaJack/honig/pull/262))

### Fixed

#### Runtime

- **Breaking:** Return an error of the runtime from Pkce::new ([#90](https://github.com/AlphaJack/honig/pull/90))
- Refuse an idn-hostname label that holds punctuation or symbols ([#130](https://github.com/AlphaJack/honig/pull/130))
- Hold a streamed request body to minItems, maxItems and uniqueItems ([#88](https://github.com/AlphaJack/honig/pull/88))
- Title a status without a reason phrase Error ([#227](https://github.com/AlphaJack/honig/pull/227))
- Check a multipleOf of any length exactly ([#96](https://github.com/AlphaJack/honig/pull/96))
- Refuse a value at a reference that resolves to nothing and at a JSON part that does not parse ([#95](https://github.com/AlphaJack/honig/pull/95))

#### Generator

- **Breaking:** Name union variants by their sorted branch schemas, not by document order ([#245](https://github.com/AlphaJack/honig/pull/245))
- **Breaking:** Refuse a pattern that does not compile ([#63](https://github.com/AlphaJack/honig/pull/63))
- Keep the extra derives and deprecated of an object type ([#263](https://github.com/AlphaJack/honig/pull/263))
- Stop spreading an allOf over a union that is its own branch ([#259](https://github.com/AlphaJack/honig/pull/259))
- Try anyOf branches in variant name order, not document order ([#244](https://github.com/AlphaJack/honig/pull/244))
- Number colliding names by their wire names, not by document order ([#243](https://github.com/AlphaJack/honig/pull/243))
- Refuse two security schemes that name the same Rust item ([#242](https://github.com/AlphaJack/honig/pull/242))
- Refuse to build a checked value that does not write out as JSON ([#235](https://github.com/AlphaJack/honig/pull/235))
- Check minProperties and maxProperties of a struct ([#207](https://github.com/AlphaJack/honig/pull/207))
- Send a multipart part as the contentMediaType of its schema ([#206](https://github.com/AlphaJack/honig/pull/206))
- Follow a ref chain of any length in description checks ([#205](https://github.com/AlphaJack/honig/pull/205))
- Bind a dynamic anchor through any chain of generics ([#204](https://github.com/AlphaJack/honig/pull/204))
- Type a schema nested too deep as a named type instead of any value ([#203](https://github.com/AlphaJack/honig/pull/203))
- Keep the properties of a site that binds a generic ([#198](https://github.com/AlphaJack/honig/pull/198))
- Check contains and dependentRequired without type beside an implied shape ([#183](https://github.com/AlphaJack/honig/pull/183))
- Check an if/then without type: object instead of dropping it ([#181](https://github.com/AlphaJack/honig/pull/181))
- Leave the checks of a type that checks itself to it ([#168](https://github.com/AlphaJack/honig/pull/168))
- Judge unevaluatedProperties with what a reference and the branches evaluate ([#137](https://github.com/AlphaJack/honig/pull/137))
- Type an enum of values of several kinds as any value ([#129](https://github.com/AlphaJack/honig/pull/129))
- Type a $ref beside an extension ([#111](https://github.com/AlphaJack/honig/pull/111))
- Leave out a readOnly property behind a $ref of a request checked whole ([#86](https://github.com/AlphaJack/honig/pull/86))

### Optimized

- Read a JSON body once for the checks of a checked type ([#221](https://github.com/AlphaJack/honig/pull/221))
- Answer yes-or-no checks without building an error message ([#211](https://github.com/AlphaJack/honig/pull/211))

#### Runtime

- Read a form's fields in one pass, in their order ([#268](https://github.com/AlphaJack/honig/pull/268))
- Read the field order of a form or object parameter without writing JSON ([#210](https://github.com/AlphaJack/honig/pull/210))
- Check multipleOf of a number without its decimal text when whole arithmetic decides it ([#193](https://github.com/AlphaJack/honig/pull/193))
- Check email, hostname, date-time, ipv4 and duration without allocating ([#190](https://github.com/AlphaJack/honig/pull/190))
- Check multipleOf and a decimal range without allocating ([#188](https://github.com/AlphaJack/honig/pull/188))

#### Generator

- Write a tagged union branch whose tag is a const without a value tree ([#256](https://github.com/AlphaJack/honig/pull/256))
- Leave a generated file alone when its text is unchanged ([#241](https://github.com/AlphaJack/honig/pull/241))
- Decode the subschemas of a check from one borrowed buffer of the value ([#194](https://github.com/AlphaJack/honig/pull/194))
- Find twin types by the hash of their key ([#145](https://github.com/AlphaJack/honig/pull/145))
- Hold a property that two allOf members declare as the allOf of both ([#110](https://github.com/AlphaJack/honig/pull/110))
- Type a one-value enum as a constant ([#108](https://github.com/AlphaJack/honig/pull/108))
- Count the branches of a oneOf in its union ([#107](https://github.com/AlphaJack/honig/pull/107))
- Judge an anyOf by its variants ([#106](https://github.com/AlphaJack/honig/pull/106))
- Find the JSON kind of each type once when emitting ([#135](https://github.com/AlphaJack/honig/pull/135))
- Keep the JSON kind of each type once it is final ([#134](https://github.com/AlphaJack/honig/pull/134))
- Find which types hold a decimal once per description ([#132](https://github.com/AlphaJack/honig/pull/132))
- Keep each schema that a whole check reaches once per module ([#89](https://github.com/AlphaJack/honig/pull/89))

### Changed

- **Breaking:** Give a decoded request no status instead of status 0 ([#251](https://github.com/AlphaJack/honig/pull/251))

#### Runtime

- **Breaking:** Rename the error types after RFC 9457 and the client ([#226](https://github.com/AlphaJack/honig/pull/226))
- **Breaking:** Drop the JSON Schema interpreter ([#179](https://github.com/AlphaJack/honig/pull/179))
- **Breaking:** Rename the mail and host name types EmailText and HostnameText ([#126](https://github.com/AlphaJack/honig/pull/126))
- **Breaking:** Check a CIDR block without ipnet ([#112](https://github.com/AlphaJack/honig/pull/112))
- **Breaking:** Name the response of a stream method that is read whole Streamed::Whole ([#104](https://github.com/AlphaJack/honig/pull/104))
- **Breaking:** Rename the conditional-request validators and the format types ([#94](https://github.com/AlphaJack/honig/pull/94))
- **Breaking:** Keep the reason in the wide event and a violation in BuildError ([#228](https://github.com/AlphaJack/honig/pull/228))
- **Breaking:** Hold mail addresses and host names as Formatted text ([#97](https://github.com/AlphaJack/honig/pull/97))

#### Generator

- **Breaking:** Drop the JSON Schema interpreter of the generator ([#214](https://github.com/AlphaJack/honig/pull/214))
- **Breaking:** Check a closed tuple once, by its Rust tuple type ([#196](https://github.com/AlphaJack/honig/pull/196))

## [0.0.3](https://github.com/AlphaJack/honig/compare/v0.0.2...v0.0.3) - 2026-11-18

### Added

#### Generator

- Warn that a pattern beside a typed format is not checked ([#78](https://github.com/AlphaJack/honig/pull/78))

### Fixed

#### Runtime

- Check decimal ranges and multipleOf exactly on decimal text ([#77](https://github.com/AlphaJack/honig/pull/77))
- Name the field of a form body that does not decode ([#79](https://github.com/AlphaJack/honig/pull/79))

#### Generator

- Find the kind and size of each type once ([#84](https://github.com/AlphaJack/honig/pull/84))
- Hold decimal128 to the limits of IEEE 754 decimal128 ([#82](https://github.com/AlphaJack/honig/pull/82))
- Put each schema that a whole check reaches in $defs once ([#81](https://github.com/AlphaJack/honig/pull/81))
- Print the body of a generated macro as items ([#80](https://github.com/AlphaJack/honig/pull/80))
- Check integer bounds and multipleOf exactly ([#76](https://github.com/AlphaJack/honig/pull/76))
- Check a cycle past the depth limit and warn where a whole schema stops ([#75](https://github.com/AlphaJack/honig/pull/75))
- Key a cycle of a schema checked whole by its pointer ([#74](https://github.com/AlphaJack/honig/pull/74))
- Rewrite the $ref of a property named default, enum or example ([#73](https://github.com/AlphaJack/honig/pull/73))
- Let a binary that denies allow_attributes take the generated code ([#71](https://github.com/AlphaJack/honig/pull/71))

## [0.0.2](https://github.com/AlphaJack/honig/compare/v0.0.1...v0.0.2) - 2026-11-17

### Added

#### Runtime

- Export tower's Service trait ([#36](https://github.com/AlphaJack/honig/pull/36))

#### CLI

- Add mod_rs to write the root of a module beside its folder ([#40](https://github.com/AlphaJack/honig/pull/40))

### Fixed

#### Runtime

- Validate the text of a library type in each item of a streamed request ([#53](https://github.com/AlphaJack/honig/pull/53))
- Validate the address of a CIDR block as ipv4 and ipv6 do ([#42](https://github.com/AlphaJack/honig/pull/42))
- Decode JSON as deep in debug builds as in release ([#37](https://github.com/AlphaJack/honig/pull/37))
- Name the field of the closest branch when no union branch matches ([#31](https://github.com/AlphaJack/honig/pull/31))

#### Generator

- Validate the text of a library type in a request parameter or body ([#51](https://github.com/AlphaJack/honig/pull/51))
- Validate the text of a library type in a union branch or a newtype of a request ([#44](https://github.com/AlphaJack/honig/pull/44))
- Validate the text of a library type in a field before the library reads it ([#43](https://github.com/AlphaJack/honig/pull/43))
- Resolve a pointer only through RFC 6901 array indexes ([#39](https://github.com/AlphaJack/honig/pull/39))
- Keep the offset of format: time in runtime::FullTime ([#38](https://github.com/AlphaJack/honig/pull/38))
- Leave a body of your own type to that type ([#34](https://github.com/AlphaJack/honig/pull/34))
- Keep each file's servers on a shared path ([#25](https://github.com/AlphaJack/honig/pull/25))

### Optimized

#### Runtime

- Look up only the keywords a schema object holds ([#33](https://github.com/AlphaJack/honig/pull/33))

## [0.0.1](https://github.com/AlphaJack/honig/releases/tag/v0.0.1) - 2026-11-16

### Added

- Publish honig

### Fixed

- Join refs with / on Windows and repair CI ([#1](https://github.com/AlphaJack/honig/pull/1))
