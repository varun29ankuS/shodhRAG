# shodh-ontology

The explicit, versioned ontology that every structured thing in Shodh is typed against:
records, entities, graph edges, memory statements, snippets and research results. Untyped
extraction is not allowed anywhere. The one sanctioned escape hatch is the `Note` class,
which is free text and searchable.

This crate is pure Rust (serde, toml, thiserror, regex, semver, chrono). It is **not** an
RDF/OWL runtime. Queries run through typed tools over LanceDB and SQLite. OWL/Turtle is an
interchange format: **export is implemented, import is not** (that is planned separately
and must report every construct it skips).

## Layout

```
ontology/core.toml            built-in core (shodh.core 1.0.0)
ontology/packs/research.toml  built-in research pack (shodh.research 1.0.0)
src/model.rs      Ontology, Class, Property, Range, Datatype, Cardinality, Dynamics
src/loader.rs     OntologyBuilder: parse, merge, validate, compile
src/statement.rs  Statement, Provenance, ValidStatement, Violation, Ontology::validate
src/supersede.rs  Ontology::supersedes -> SupersedeDecision
src/slice.rs      Ontology::slice_for -> OntologySlice, render_prompt
src/diff.rs       OntologyDiff::between, Compatibility
src/turtle.rs     Ontology::to_turtle, Ontology::statements_to_turtle
```

## Authoring

An ontology is a set of TOML sources. Each source declares exactly one header:

- `[core]`: exactly one per compiled ontology;
- `[pack]`: an opt-in domain pack;
- `[extension]`: a workspace extension.

The header holds `id`, `version` (semver), `label`, `prefix` and `namespace` (an IRI ending
in `#` or `/`). It may also hold `requires = { "shodh.core" = "^1.0" }`.

```toml
[[class]]
id = "Invoice"                       # UpperCamelCase, unique across all terms
parent = "Document"                  # single inheritance; defaults to Thing
description = "A bill issued by a seller ..."
identity_keys = [["invoiceNumber", "issuedBy"]]   # list of keys; a key is a property list
cue_terms = ["invoice", "tax invoice"]            # case-insensitive, word-bounded
cue_patterns = ['INV-[0-9]+']                     # regex
equivalent_to = ["https://schema.org/Invoice"]
[class.dynamics]
reinforcement = 0.0

[[property]]
id = "gstin"                         # lowerCamelCase
description = "15-character GSTIN"
domain = "TaxId"                     # or a list: ["Task", "Obligation"]
range = "String"                     # class id or datatype
cardinality = "one"                  # "one" | "many"
required = false
temporal = false                     # only with "one": newer value supersedes older
pattern = '[0-9]{2}[A-Z]{5}[0-9]{4}[A-Z][1-9A-Z]Z[0-9A-Z]'
pattern_is_cue = true                # a match in text selects the class when slicing
```

**Datatypes**

| Datatype | Accepted values |
|---|---|
| `String` | Any text. |
| `Boolean` | A boolean. |
| `Integer` | A whole number. |
| `Decimal` | `-?digits[.digits]`. Separators and exponents are rejected. |
| `Money` | A decimal plus a 3-letter upper-case ISO 4217 code, given either as `{amount, currency}` or as the text `"1250.00 INR"`. The code's shape is checked; the code list itself is not. |
| `Date` | Strict `YYYY-MM-DD`, so `2024-1-5` and `2024-02-30` are rejected. |
| `DateTime` | RFC 3339 with an explicit offset. A local time without an offset is ambiguous and is rejected. |
| `Url` | An absolute URL with a scheme. |
| `Email` | An e-mail address. |
| `Enum` | Exactly one of the strings in `values`. |

A **regex-constrained value** is written as `range = "String"` (or `Url`/`Email`) plus a
`pattern`. The pattern must match the whole value; anchors are added automatically.

**Merge rules.** These are enforced by `OntologyBuilder::build`, and every error carries
`file:line:column`:

- Sources merge in this order: core, then packs, then extensions. Within each layer they
  merge in the order they were added.
- Packs and extensions can only **add**. Redefining any existing class or property is an
  error, for example:

  ```text
  workspace/ext.toml:8:6: `Invoice` is already defined at ontology/core.toml:111:6; packs and extensions may only add terms, never redefine them
  ```

- A term may only reference terms from its own source or an earlier one.
- An extension may add optional properties to core classes. It may **not** add a
  *required* property to a class it does not own, because that would retroactively
  invalidate statements already accepted.
- Every malformed reference, id, range, enum, pattern, cycle, namespace and dynamics
  table is reported. Errors are collected, not returned one at a time.

```rust
let ontology = OntologyBuilder::new()
    .with_core()
    .with_builtin_pack("research")
    .add_dir("workspace/ontology")      // *.toml, name order
    .build()?;                          // Err(LoadErrors) lists every problem
```

## Statements and validation

A fact is an n-ary **statement**, not a bare triple. It has these fields:

- `id`: makes the statement addressable and citable.
- `class`
- `subject`: an optional resolved entity.
- `properties`: a map of `RawValue`.
- `ontology_version`: the version of the source (core, pack or extension) that defines
  `class`.
- `valid_from`: optional. If it is absent, the extraction time is used.
- `provenance`: holds `source`, `generation`, `page`, `span`, the extractor (`rule`,
  `gliner`, `llm` or `user`, with its `version`), `confidence` and `extracted_at`.

`id`, `subject` and `valid_from` go beyond the spec's minimal shape. They are needed for
addressability and supersede.

`Ontology::validate(&Statement) -> Result<ValidStatement, Vec<Violation>>` checks the
following and reports **all** violations:

- provenance is present and well formed;
- the version is compatible with the loaded version of the source that defines the
  class (caret: a statement written under `1.2.0` is readable by `1.x` with `x >= 2`);
- the class exists;
- every property exists and applies to the class (inheritance included);
- relations are entity references whose class is within the range;
- datatypes parse strictly, and patterns and enum membership hold;
- `one` properties have at most one value;
- required properties are present.

Values are parsed, never coerced. Callers should drop and count violations per class using
`Violation::code()`. Only `validate` can construct a `ValidStatement`.

## Supersede semantics

`Ontology::supersedes(existing, incoming) -> SupersedeDecision` is a pure, deterministic
function. No LLM is involved.

1. **Classes must be related**: the same class, or one a subclass of the other. Otherwise
   the result is `Independent(UnrelatedClasses)`.
2. **Same subject** means one of two things:
   - both statements carry the same `subject` id;
   - or, for some identity key, every key property has a shared value.

   If a key is present on both sides but differs, the result is `DifferentSubjects`. With
   no basis for comparison, the result is `UnknownIdentity`.
3. Each property the incoming statement carries is then handled as follows:

| Property | Outcome |
|---|---|
| `many` | Values not already present are `Added`; otherwise the property is `Unchanged`. |
| `one` + `temporal` | The newer value is current. The older value is closed with `valid_to` and kept as history (`Superseded`). If the incoming statement is older, its value is recorded as `Historical` instead. Time is `valid_from`, falling back to `extracted_at`. A tie goes to the incoming statement. |
| `one`, not temporal | A different value is a `Conflict`, for example two totals for one invoice. It is never overwritten silently. |

Equal values are compared semantically. For example, `12500.00 INR` equals `12500 INR`.

## Memory dynamics

Each class carries `Dynamics`:

- `decay_half_life_days`: `None` means no decay.
- `reinforcement`: the fraction of headroom restored when a memory is recalled or
  re-asserted, in `[0, 1]`.
- `expires_after`: either `{ days }`, or `{ property, grace_days }` on a single-valued
  `Date`/`DateTime` property.

A class without a `[class.dynamics]` table inherits its parent's dynamics. A declared table
replaces them as a whole.

Core defaults:

| Classes | Half-life | Reinforcement | Expiry |
|---|---|---|---|
| Parties and identity | 10 years | 0.05 | |
| Preferences | 2 years | 0.2 | |
| Decisions | 5 years | 0.1 | |
| Procedures | 1 year | 0.3 (highest) | |
| Episodes | 14 days | 0.1 | |
| Tasks | 90 days | 0.1 | `dueOn` + 30 days |
| Events | 30 days | 0.1 | `endsAt` + 30 days |
| Obligations | No decay | 0 | `dueOn` + 90 days |
| Records (Document, Invoice, Contract, Payment) | No decay | 0 | |

## Slicing

`Ontology::slice_for(text)` selects classes in three ways:

- their cue terms, which are word-bounded, so `pan` does not match inside "Japan";
- their cue patterns;
- the value patterns of properties marked `pattern_is_cue`, for example a bare GSTIN
  selects `TaxId`.

It then adds those classes' ancestors (for inherited properties) and the classes their
relations point to. `OntologySlice::render_prompt()` produces a compact, deterministic
schema for a constrained extraction step: each matched class with every applicable
property, its range, cardinality, flags and pattern. Loose patterns, such as postal codes
or language tags, are deliberately not cues: they match ordinary words.

## Versioning

`OntologyDiff::between(old, new)` lists added, removed and changed classes and properties
(with the changed fields). It classifies the change as one of:

- `Identical`
- `Cosmetic`: labels, descriptions, equivalences or dynamics.
- `Additive`: new terms, or changed cues or identity keys.
- `Breaking`: removals; changes to domain, range, cardinality, temporal, required, pattern
  or parent; or a new required property on an existing class.

It also computes `affected_classes`, sorted and including subclasses. These are the
classes whose documents must be re-extracted as a new generation. Changes to dynamics or
documentation never trigger re-extraction.

Every source (core, each pack, each extension) is versioned independently. The diff
attributes each change to the source that defines the term and reports a `SourceChange`
per source. `version_bump_sufficient()` holds only if every source moved enough for its
own changes:

- cosmetic: a patch bump;
- additive: a minor bump;
- breaking: a major bump, or a minor bump for `0.x`;
- a newly added source is always sufficient; a removed source never is.

Statements record the version of the source that defines their class, so a breaking
change to a pack class is caught at validation without touching the core version.

## Export

`Ontology::to_turtle()` emits OWL 2 in Turtle:

| Ontology construct | OWL output |
|---|---|
| Class | `owl:Class` with labels and comments, `rdfs:subClassOf` and `owl:equivalentClass` |
| Identity keys | `owl:hasKey`, one per identity key |
| Relation | `owl:ObjectProperty` |
| `Money` | `owl:ObjectProperty` ranging over `schema:MonetaryAmount` |
| Other attributes | `owl:DatatypeProperty` |
| Multi-class domain | `owl:unionOf` |
| `Enum` | `owl:oneOf` |
| `pattern` | `xsd:pattern` datatype restriction |
| `one` + required | `owl:cardinality 1` restriction on each domain class |
| `one` | `owl:maxCardinality 1` restriction on each domain class |
| `many` + required | `owl:minCardinality 1` restriction on each domain class |

Dynamics, cue terms and the temporal flag have no OWL counterpart and are not exported.

`Ontology::statements_to_turtle()` emits each statement as a `prov:Entity` typed with its
class, with:

- `prov:wasDerivedFrom` (the source);
- `prov:generatedAtTime`;
- `prov:wasGeneratedBy`: an activity associated with a `prov:SoftwareAgent`, or a
  `prov:Person` for user-stated facts.

Page, span, generation, confidence and extractor kind and version use the
`sprov:` (`urn:shodh:prov#`) namespace. Shodh namespaces are `urn:` IRIs, so the export
does not claim a web domain. Output is deterministic and has golden tests.

## How the rest of Shodh consumes it (next PRs)

| Consumer | Use |
|---|---|
| **Records (M4)** | `RecordExtractor` targets ontology classes. `query_records` specs are validated against `properties_of(class)`. Coverage counts documents lacking a `required` property. |
| **Knowledge graph** | Deterministic extractors and GLiNER2 labels are generated from classes and patterns. Edges are relation properties. Entity resolution uses `identity_keys_of`. Only ids from the ontology are allowed. |
| **Memory** (LanceDB + SQLite) | Memories are `ValidStatement`s. `supersedes` drives history (`valid_to`). `Dynamics` drives decay, reinforcement and expiry. `Note` is the untyped escape hatch. |
| **Snippets and research** | The research pack's `Snippet` (image ref, text, page, rect) and n-ary `Result` statements are first-class and cite their source span through provenance. |
| **Agent tools** | `describe_ontology` renders classes. Constrained LLM extraction receives `slice_for(chunk).render_prompt()`. Proposed extensions are validated by the loader before user approval. |
