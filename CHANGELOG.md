# Changelog

## [0.0.4](https://github.com/AlphaJack/honig/compare/v0.0.3...v0.0.4) - 2026-11-28

### Added

- **Breaking:** Pick format types with runtime features and never edit Cargo.toml ([#285](https://github.com/AlphaJack/honig/pull/285))
- **Breaking:** Check each credential with a verify method of its scheme on the Api ([#229](https://github.com/AlphaJack/honig/pull/229))
- **Breaking:** Fill only RFC 9457 error bodies, and map any other in the Api's error_response ([#215](https://github.com/AlphaJack/honig/pull/215))

#### Runtime

- **Breaking:** Add a Decimal128 type for a decimal128 number ([#498](https://github.com/AlphaJack/honig/pull/498))
- **Breaking:** Hand the wide event to layers in the response ([#408](https://github.com/AlphaJack/honig/pull/408))
- **Breaking:** Require authenticate on a server whose description declares security ([#224](https://github.com/AlphaJack/honig/pull/224))
- **Breaking:** Hold a time with its offset in plain numbers ([#113](https://github.com/AlphaJack/honig/pull/113))
- **Breaking:** Validate the text of a library type in responses too ([#59](https://github.com/AlphaJack/honig/pull/59))
- Send an own JSON error body with Rejection::json ([#232](https://github.com/AlphaJack/honig/pull/232))
- Hold a sequence to minItems, maxItems and uniqueItems as it arrives ([#68](https://github.com/AlphaJack/honig/pull/68))
- Add Blob::try_stream for a body that can fail partway ([#507](https://github.com/AlphaJack/honig/pull/507))
- Name the scopes that a 403 for insufficient_scope requires ([#506](https://github.com/AlphaJack/honig/pull/506))

#### Generator

- **Breaking:** Rename the Otherwise variant of a conditional schema to Else ([#497](https://github.com/AlphaJack/honig/pull/497))
- **Breaking:** Rename keep_unknown_fields to keep_unknown_properties ([#494](https://github.com/AlphaJack/honig/pull/494))
- **Breaking:** Place the root of a module by mod_rs alone ([#486](https://github.com/AlphaJack/honig/pull/486))
- **Breaking:** Give into_result a Success struct for any body with headers ([#457](https://github.com/AlphaJack/honig/pull/457))
- **Breaking:** Number pattern maps in the order of their pattern text ([#407](https://github.com/AlphaJack/honig/pull/407))
- **Breaking:** Give into_result a Success struct when two values share a type ([#404](https://github.com/AlphaJack/honig/pull/404))
- **Breaking:** Give each operation its security as a typed value ([#379](https://github.com/AlphaJack/honig/pull/379))
- **Breaking:** Type the OAuth scopes of the client ([#345](https://github.com/AlphaJack/honig/pull/345))
- **Breaking:** Give a range or default response a status type of its own codes ([#343](https://github.com/AlphaJack/honig/pull/343))
- **Breaking:** Build a bounded integer in a constant with new ([#338](https://github.com/AlphaJack/honig/pull/338))
- **Breaking:** Remove the chrono date types ([#280](https://github.com/AlphaJack/honig/pull/280))
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
- Construct a response from each of its values ([#473](https://github.com/AlphaJack/honig/pull/473))
- Turn a string enum into its text with into and as_ref ([#470](https://github.com/AlphaJack/honig/pull/470))
- Build a length-checked string in a constant ([#467](https://github.com/AlphaJack/honig/pull/467))
- Name the setter to call when an operation lacks credentials ([#406](https://github.com/AlphaJack/honig/pull/406))
- Name integer enum values with an object of x-enum-varnames ([#400](https://github.com/AlphaJack/honig/pull/400))
- Give a bounded checked integer a const constructor that fails a build on a literal outside its limits ([#329](https://github.com/AlphaJack/honig/pull/329))
- Give each operation a Result alias that its handler returns ([#281](https://github.com/AlphaJack/honig/pull/281))
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

- **Breaking:** Check a module with generate --check ([#488](https://github.com/AlphaJack/honig/pull/488))
- **Breaking:** Drop upgrade and bundle, and name the manifest flag --manifest ([#485](https://github.com/AlphaJack/honig/pull/485))
- **Breaking:** List the operations of a module once, for both halves ([#262](https://github.com/AlphaJack/honig/pull/262))
- Generate before the honig dependency is added ([#491](https://github.com/AlphaJack/honig/pull/491))
- Fail on a warning with --deny-warnings ([#490](https://github.com/AlphaJack/honig/pull/490))

### Fixed

#### Runtime

- **Breaking:** Answer 400 for a query string that does not decode ([#351](https://github.com/AlphaJack/honig/pull/351))
- **Breaking:** Answer 400 for a parameter that does not decode in its style ([#350](https://github.com/AlphaJack/honig/pull/350))
- **Breaking:** Refuse a body that is not UTF-8 and a JSON header that does not parse ([#349](https://github.com/AlphaJack/honig/pull/349))
- **Breaking:** Route a request once, so that a layer cannot send it to another operation ([#288](https://github.com/AlphaJack/honig/pull/288))
- Refuse an idn-hostname label that holds punctuation or symbols ([#130](https://github.com/AlphaJack/honig/pull/130))
- Hold a streamed request body to minItems, maxItems and uniqueItems ([#88](https://github.com/AlphaJack/honig/pull/88))
- Answer 500 when a handler panics ([#459](https://github.com/AlphaJack/honig/pull/459))
- Document honig with its default features ([#421](https://github.com/AlphaJack/honig/pull/421))
- Export the browser transport, MaybeSend and MaybeSync ([#417](https://github.com/AlphaJack/honig/pull/417))
- Hide OAuth secrets from Debug ([#412](https://github.com/AlphaJack/honig/pull/412))

#### Generator

- **Breaking:** Give the response itself from into_result of a range ([#475](https://github.com/AlphaJack/honig/pull/475))
- **Breaking:** Put the client id first in every OAuth flow ([#472](https://github.com/AlphaJack/honig/pull/472))
- **Breaking:** Refuse two values of one type by position through aliases ([#471](https://github.com/AlphaJack/honig/pull/471))
- **Breaking:** Fail on a schema that accepts no value through false ([#466](https://github.com/AlphaJack/honig/pull/466))
- **Breaking:** Fail on an empty enum ([#463](https://github.com/AlphaJack/honig/pull/463))
- **Breaking:** Fail on a oneOf whose every branch has a twin ([#456](https://github.com/AlphaJack/honig/pull/456))
- **Breaking:** End the evaluation of a schema that applies itself ([#451](https://github.com/AlphaJack/honig/pull/451))
- **Breaking:** Check the format of a text body ([#389](https://github.com/AlphaJack/honig/pull/389))
- **Breaking:** Infer a union tag only from a required property ([#383](https://github.com/AlphaJack/honig/pull/383))
- **Breaking:** Keep else when several conditionals test one property ([#382](https://github.com/AlphaJack/honig/pull/382))
- **Breaking:** Apply then when the property an if tests is absent ([#381](https://github.com/AlphaJack/honig/pull/381))
- **Breaking:** Warn about a link pointer that reaches nothing and XML arrays of one item name ([#353](https://github.com/AlphaJack/honig/pull/353))
- **Breaking:** Fail generation for a limit that no check would run ([#352](https://github.com/AlphaJack/honig/pull/352))
- **Breaking:** Refuse an anchor that one resource declares twice ([#348](https://github.com/AlphaJack/honig/pull/348))
- **Breaking:** Fail generation for discriminated branches that only an unpinned tag separates ([#347](https://github.com/AlphaJack/honig/pull/347))
- **Breaking:** Fail generation for an anyOf whose objects cannot be told apart ([#346](https://github.com/AlphaJack/honig/pull/346))
- **Breaking:** Fail generation where a keyword would lose its meaning ([#342](https://github.com/AlphaJack/honig/pull/342))
- **Breaking:** Report every error of a description in one run ([#341](https://github.com/AlphaJack/honig/pull/341))
- **Breaking:** Fail generation on any error of the description ([#334](https://github.com/AlphaJack/honig/pull/334))
- **Breaking:** Fail generation for two templated paths that are the same ([#333](https://github.com/AlphaJack/honig/pull/333))
- **Breaking:** Fail generation for merged files that differ in dialect or identity and for an overlay of another description ([#326](https://github.com/AlphaJack/honig/pull/326))
- **Breaking:** Fail generation for a link that sets a parameter its target lacks ([#324](https://github.com/AlphaJack/honig/pull/324))
- **Breaking:** Fail generation for a default that its own schema refuses ([#320](https://github.com/AlphaJack/honig/pull/320))
- **Breaking:** Give into_result no tuple of two values of one type ([#318](https://github.com/AlphaJack/honig/pull/318))
- **Breaking:** Fail generation for an enum or const value outside its type ([#310](https://github.com/AlphaJack/honig/pull/310))
- **Breaking:** Fail generation for a derives key that names no type ([#317](https://github.com/AlphaJack/honig/pull/317))
- **Breaking:** Derive Default only for a struct with no required field ([#315](https://github.com/AlphaJack/honig/pull/315))
- **Breaking:** Fail generation for an unknown operation in the operations list ([#316](https://github.com/AlphaJack/honig/pull/316))
- **Breaking:** Fail generation when enum names do not pair with the values ([#308](https://github.com/AlphaJack/honig/pull/308))
- **Breaking:** Fail generation for a schema that no value of its type meets ([#309](https://github.com/AlphaJack/honig/pull/309))
- **Breaking:** Fail on two names that are one item in Rust and number media variants by media type ([#306](https://github.com/AlphaJack/honig/pull/306))
- **Breaking:** Refuse a server from a description with an unknown field ([#301](https://github.com/AlphaJack/honig/pull/301))
- **Breaking:** Fail generation when an overlay action selects no nodes ([#304](https://github.com/AlphaJack/honig/pull/304))
- **Breaking:** Name union variants by their sorted branch schemas, not by document order ([#245](https://github.com/AlphaJack/honig/pull/245))
- **Breaking:** Refuse a pattern that does not compile ([#63](https://github.com/AlphaJack/honig/pull/63))
- **Breaking:** Name the 2xx class Successful ([#474](https://github.com/AlphaJack/honig/pull/474))
- Say that keep_unknown_properties keeps an anyOf tie whole ([#505](https://github.com/AlphaJack/honig/pull/505))
- Keep the properties of every matching anyOf branch ([#499](https://github.com/AlphaJack/honig/pull/499))
- Apply an object or array default ([#495](https://github.com/AlphaJack/honig/pull/495))
- Warn when an XML body stays raw text ([#477](https://github.com/AlphaJack/honig/pull/477))
- Fail only on a schema that its value must meet ([#465](https://github.com/AlphaJack/honig/pull/465))
- Import the operations only where a client or server refers to them ([#461](https://github.com/AlphaJack/honig/pull/461))
- Name the $id that a reference resolves against ([#455](https://github.com/AlphaJack/honig/pull/455))
- Allow dead code in the server of a binary ([#454](https://github.com/AlphaJack/honig/pull/454))
- Make the security of an operation Clone ([#453](https://github.com/AlphaJack/honig/pull/453))
- Check an enum of null alone beside another keyword ([#432](https://github.com/AlphaJack/honig/pull/432))
- Print the bodies of builder and list macros one entry per line ([#420](https://github.com/AlphaJack/honig/pull/420))
- Hide the credentials of a client from Debug ([#413](https://github.com/AlphaJack/honig/pull/413))
- Warn about a required Authorization header parameter instead of failing ([#405](https://github.com/AlphaJack/honig/pull/405))
- Refuse integer bounds that its format or multipleOf leaves empty ([#403](https://github.com/AlphaJack/honig/pull/403))
- Encode a response from a borrow unless a body moves out ([#401](https://github.com/AlphaJack/honig/pull/401))
- Show a whole number of a wrong-type enum as the description wrote it ([#399](https://github.com/AlphaJack/honig/pull/399))
- Warn about an ignored Accept or Content-Type header parameter ([#398](https://github.com/AlphaJack/honig/pull/398))
- Report a scheme listed by name and by URI once ([#397](https://github.com/AlphaJack/honig/pull/397))
- Point at the second scope of two that are one variant ([#396](https://github.com/AlphaJack/honig/pull/396))
- Validate every subschema and let a closed branch overlap ([#394](https://github.com/AlphaJack/honig/pull/394))
- Point each diagnostic at its keyword ([#393](https://github.com/AlphaJack/honig/pull/393))
- Name an operation server by its URL when it has no name ([#392](https://github.com/AlphaJack/honig/pull/392))
- Treat a 2XX response as a success ([#391](https://github.com/AlphaJack/honig/pull/391))
- Compile a checked integer whose bound is outside its type ([#390](https://github.com/AlphaJack/honig/pull/390))
- Honor a 2.0 collectionFormat on header and path arrays ([#388](https://github.com/AlphaJack/honig/pull/388))
- Keep the collectionFormat of a 2.0 form array ([#387](https://github.com/AlphaJack/honig/pull/387))
- Warn instead of failing for a schema that no check enforces ([#358](https://github.com/AlphaJack/honig/pull/358))
- Check the limits beside x-rust-type instead of failing ([#357](https://github.com/AlphaJack/honig/pull/357))
- Check a length or pattern on base64 bytes instead of failing ([#356](https://github.com/AlphaJack/honig/pull/356))
- Check a pattern beside a typed format instead of failing ([#355](https://github.com/AlphaJack/honig/pull/355))
- Warn instead of failing for alike union branches and a 3.1 nullable ([#354](https://github.com/AlphaJack/honig/pull/354))
- Check the limits of a text body ([#335](https://github.com/AlphaJack/honig/pull/335))
- Refuse a server again when a scheme refers to no scheme ([#332](https://github.com/AlphaJack/honig/pull/332))
- Warn about a keyword that is left out next to a 3.0 ref ([#330](https://github.com/AlphaJack/honig/pull/330))
- Warn about a limit on a text or binary body that no check runs ([#328](https://github.com/AlphaJack/honig/pull/328))
- Warn about a default that no field of its type can have ([#327](https://github.com/AlphaJack/honig/pull/327))
- Refuse a server for a security scheme that it would read as another ([#323](https://github.com/AlphaJack/honig/pull/323))
- Answer 500 when a handler returns a status that its response variant does not stand for ([#319](https://github.com/AlphaJack/honig/pull/319))
- Keep every limit of a then beside those of the base schema ([#321](https://github.com/AlphaJack/honig/pull/321))
- Delete only generated files that a run no longer writes ([#312](https://github.com/AlphaJack/honig/pull/312))
- Refuse a server from a required Authorization header parameter ([#307](https://github.com/AlphaJack/honig/pull/307))
- Refuse a server for two scopes that are one variant of the scope enum ([#305](https://github.com/AlphaJack/honig/pull/305))
- Find security requirements by structure, not by key name ([#299](https://github.com/AlphaJack/honig/pull/299))
- Refuse a server for two webhooks or callbacks of one name and method ([#302](https://github.com/AlphaJack/honig/pull/302))
- Refuse a requirement that lists a scheme by name and by URI with different scopes ([#300](https://github.com/AlphaJack/honig/pull/300))
- Refuse a server when a security is not a list ([#295](https://github.com/AlphaJack/honig/pull/295))
- Keep a merged file's security on its component path items and callbacks ([#297](https://github.com/AlphaJack/honig/pull/297))
- Refuse a server when a scheme refers to no scheme ([#296](https://github.com/AlphaJack/honig/pull/296))
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

#### CLI

- Accept a path dependency on honig without a version ([#500](https://github.com/AlphaJack/honig/pull/500))
- Print every feature that the description needs from init ([#469](https://github.com/AlphaJack/honig/pull/469))
- Accept the caret pin that cargo add writes on a 0.0 version ([#468](https://github.com/AlphaJack/honig/pull/468))

### Optimized

- Read a JSON body once for the checks of a checked type ([#221](https://github.com/AlphaJack/honig/pull/221))
- Answer yes-or-no checks without building an error message ([#211](https://github.com/AlphaJack/honig/pull/211))

#### Runtime

- Copy a collected body at most once ([#501](https://github.com/AlphaJack/honig/pull/501))
- Decide each union variant once per value ([#452](https://github.com/AlphaJack/honig/pull/452))
- Read a form's fields in one pass, in their order ([#268](https://github.com/AlphaJack/honig/pull/268))
- Buffer a request body of unknown length once at its full size ([#415](https://github.com/AlphaJack/honig/pull/415))

#### Generator

- Number a taken name without capitalizing it again ([#458](https://github.com/AlphaJack/honig/pull/458))
- Look up the twin of a type by index ([#325](https://github.com/AlphaJack/honig/pull/325))
- Count a oneOf branch of required names by its names ([#286](https://github.com/AlphaJack/honig/pull/286))
- Find the kinds of every type in constant time per lookup ([#282](https://github.com/AlphaJack/honig/pull/282))
- Check the keywords beside an implied shape on the decoded text ([#276](https://github.com/AlphaJack/honig/pull/276))
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

- **Breaking:** Name the checked text of a format `*Str` and re-export it beside the types ([#284](https://github.com/AlphaJack/honig/pull/284))
- **Breaking:** Rename violation locations, results and primitive shapes as OAS and Rust do ([#283](https://github.com/AlphaJack/honig/pull/283))
- **Breaking:** Give a decoded request no status instead of status 0 ([#251](https://github.com/AlphaJack/honig/pull/251))

#### Runtime

- **Breaking:** Rename the error types after RFC 9457 and the client ([#226](https://github.com/AlphaJack/honig/pull/226))
- **Breaking:** Drop the JSON Schema interpreter ([#179](https://github.com/AlphaJack/honig/pull/179))
- **Breaking:** Rename the mail and host name types EmailText and HostnameText ([#126](https://github.com/AlphaJack/honig/pull/126))
- **Breaking:** Check a CIDR block without ipnet ([#112](https://github.com/AlphaJack/honig/pull/112))
- **Breaking:** Name the response of a stream method that is read whole Streamed::Whole ([#104](https://github.com/AlphaJack/honig/pull/104))
- **Breaking:** Rename the conditional-request validators and the format types ([#94](https://github.com/AlphaJack/honig/pull/94))

#### Generator

- **Breaking:** Call the honig library by its path and drop runtime.rs ([#409](https://github.com/AlphaJack/honig/pull/409))
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
