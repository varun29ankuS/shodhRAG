/**
 * Mermaid sources for the repair tests (`mermaidRepair.test.ts`): diagrams
 * as weaker models write them, each with the repair expected, and valid
 * diagrams that must come back unchanged. Every broken source here is
 * rejected by the mermaid 11 parser and every expected repair is accepted.
 */

export interface BrokenFixture {
  name: string;
  source: string;
  repaired: string;
}

export const BROKEN: readonly BrokenFixture[] = [
  {
    name: 'parentheses in a box label',
    source: 'flowchart TD\n  A[Start (init)] --> B{x: y?}\n  B -->|yes| C[Done]',
    repaired: 'flowchart TD\n  A["Start (init)"] --> B{x: y?}\n  B -->|yes| C[Done]',
  },
  {
    name: 'nested parentheses in a round node and a database node',
    source: 'graph LR\n  A(Load data (CSV)) --> B[(DB (main))]',
    repaired: 'graph LR\n  A("Load data (CSV)") --> B[("DB (main)")]',
  },
  {
    name: 'quotes inside a label',
    source: 'flowchart LR\n  A[He said "hi"] --> B["She said "bye""]',
    repaired: 'flowchart LR\n  A["He said #quot;hi#quot;"] --> B["She said #quot;bye#quot;"]',
  },
  {
    name: 'brackets in a decision and in an edge label',
    source: 'flowchart TD\n  Q{Is x[0] > 1?} -->|yes (often)| R[Return {a, b}]\n  Q -- no (rare) --> S',
    repaired: 'flowchart TD\n  Q{"Is x[0] > 1?"} -->|"yes (often)"| R["Return {a, b}"]\n  Q -- "no (rare)" --> S',
  },
  {
    name: 'unicode arrows and dashes',
    source: 'flowchart LR\n  A[Input] → B[Encoder]\n  B ⇒ C[Decoder]\n  C —> D[Output]\n  D – E\n  E -> F[2019–2020 → now]',
    repaired: 'flowchart LR\n  A[Input] --> B[Encoder]\n  B ==> C[Decoder]\n  C --> D[Output]\n  D --- E\n  E --> F[2019–2020 → now]',
  },
  {
    name: 'html formatting tags',
    source: 'flowchart TD\n  A[<span style="color:red">Hot</span> <b>path</b><br/>two] --> B\n  subgraph S [My <i>Group</i> (v2)]\n    B --> C\n  end',
    repaired: 'flowchart TD\n  A[Hot path<br/>two] --> B\n  subgraph S ["My Group (v2)"]\n    B --> C\n  end',
  },
  {
    name: 'subgraph titles with punctuation',
    source: 'flowchart TB\n  subgraph Phase 1: Setup (init)\n    A --> B\n  end\n  subgraph Training, Eval\n    C --> D\n  end\n  B --> C',
    repaired: 'flowchart TB\n  subgraph "Phase 1: Setup (init)"\n    A --> B\n  end\n  subgraph "Training, Eval"\n    C --> D\n  end\n  B --> C',
  },
  {
    name: 'duplicate header lines',
    source: 'graph TD\ngraph TD\n  A --> B\n  B --> C',
    repaired: 'graph TD\n  A --> B\n  B --> C',
  },
  {
    name: 'edges with no target or no source',
    source: 'flowchart LR\n  A --> B\n  B --> C;\n  C -->\n  --> D',
    repaired: 'flowchart LR\n  A --> B\n  B --> C;\n  C\n  D',
  },
  {
    name: 'semicolons inside sequence messages and unicode arrows',
    source: 'sequenceDiagram\n  participant U as User\n  U → S: login; then fetch\n  S-->>U: token',
    repaired: 'sequenceDiagram\n  participant U as User\n  U->>S: login#59; then fetch\n  S-->>U: token',
  },
  {
    name: 'trailing semicolons in a class diagram',
    source: 'classDiagram\n  class Animal;\n  Animal <|-- Dog;\n  Animal : +String name;',
    repaired: 'classDiagram\n  class Animal\n  Animal <|-- Dog\n  Animal : +String name',
  },
  {
    name: 'trailing semicolons in an entity relationship diagram',
    source: 'erDiagram\n  CUSTOMER ||--o{ ORDER : places;\n  ORDER ||--|{ LINE : contains;',
    repaired: 'erDiagram\n  CUSTOMER ||--o{ ORDER : places\n  ORDER ||--|{ LINE : contains',
  },
  {
    name: 'trailing semicolons in a gantt chart',
    source: 'gantt\n  title Plan\n  dateFormat YYYY-MM-DD\n  section Build\n  Design :a1, 2026-01-01, 3d;\n  Code :after a1, 5d;',
    repaired: 'gantt\n  title Plan\n  dateFormat YYYY-MM-DD\n  section Build\n  Design :a1, 2026-01-01, 3d\n  Code :after a1, 5d',
  },
];

/** Valid diagrams: the repair must return each one unchanged. */
export const VALID: readonly string[] = [
  [
    'flowchart LR',
    '  %% every node shape and link style',
    '  A[Box] --> B(Round) --> C([Stadium]) --> D[[Subroutine]]',
    '  D --> E[(Database)] --> F((Circle)) --> G>Flag]',
    '  G --> H{Decision} --> I{{Hexagon}} --> J[/Lean right/]',
    '  J --> K[\\Lean left\\] --> L[/Trapezoid\\] --> M(((Double)))',
    '  A -.-> M',
    '  A ==> M',
    '  A --- M',
    '  A -- text --> M',
    '  A -->|label| M',
    '  A -->|"quoted (label)"| M',
    '  N["Already (quoted)"]:::hot --> O["#quot;entity#quot;"]',
    '  P & Q --> R',
    '  S --o T',
    '  S --x T',
    '  S <--> T',
    '  S ~~~ T',
    '  U@{ shape: rect, label: "Shape (v11)" } --> V',
    '  W[x: y; z, #1 & <2>] --> X[line<br/>break]',
    '  subgraph one [Group title]',
    '    direction TB',
    '    Y --> Z',
    '  end',
    '  subgraph "Quoted (title)"',
    '    Y2 --> Z2',
    '  end',
    '  classDef hot fill:#f96,stroke:#333;',
    '  class A,B hot',
    '  style C fill:#bbf',
    '  linkStyle 0 stroke:#f66',
    '  click A "https://example.com"',
  ].join('\n'),
  'graph TD; A-->B; B-->C',
  'graph TD\n  A-->B;\n  B-->C;',
  '---\ntitle: Pipeline\n---\nflowchart LR\n  A --> B',
  '%%{init: {"theme": "dark"}}%%\nflowchart LR\n  A --> B',
  'sequenceDiagram\n  participant A as Alice\n  A->>B: Hello (there): how are you?\n  B-->>A: Fine#59; thanks\n  Note right of B: thinks\n  loop Every minute\n    A-)B: ping\n  end',
  'classDiagram\n  class Animal {\n    +String name\n    +eat() void\n  }\n  Animal <|-- Dog\n  List~String~ <-- Animal',
  'stateDiagram-v2\n  [*] --> Idle\n  Idle --> Running : start\n  Running --> [*]',
  'erDiagram\n  CUSTOMER ||--o{ ORDER : places\n  ORDER {\n    string id PK\n  }',
  'gantt\n  title Plan\n  dateFormat YYYY-MM-DD\n  section A\n  Task one :a1, 2026-01-01, 3d',
  'pie title Pets\n  "Dogs" : 386\n  "Cats" : 85',
  'mindmap\n  root((Topic))\n    Branch (one)\n    Branch two',
  'timeline\n  title History\n  2020 : Event (one)',
  'journey\n  title Day\n  section Morning\n    Wake up: 5: Me',
];
