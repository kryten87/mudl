Mermaid diagrams
===============================================================================

Examples of mermaid diagram types rendered by mudl.


## Flowchart

```mermaid
graph TD
    A[Start] --> B{Is it working?}
    B -->|Yes| C[Great!]
    B -->|No| D[Debug]
    D --> B
```


## Sequence diagram

```mermaid
sequenceDiagram
    participant WebView
    participant Server
    participant Core

    WebView->>Server: GET /
    Server->>Core: render_up(markdown)
    Core->>Core: Parse markdown (pulldown-cmark)
    Core->>Core: Walk event stream, emit HTML
    Core-->>Server: HTML string
    Server-->>WebView: HTTP response
    WebView->>WebView: mermaid.run()
```


## State diagram

```mermaid
stateDiagram-v2
    [*] --> Up
    Up --> Down: Space bar
    Down --> Up: Space bar

    Up --> Up: file saved (auto-reload)
    Down --> Down: file saved (auto-reload)
```


## Class diagram

```mermaid
classDiagram
    class AppState {
        +Theme theme
        +Lighting lighting
        +Mode modeInActiveTab
    }
    class DocumentState {
        +Mode mode
        +toggleMode()
    }
    class FindState {
        +String searchText
        +Bool isVisible
    }
    DocumentState --> FindState
```


## Pie chart

```mermaid
pie title Lines of code
    "Rust" : 3200
    "JavaScript" : 800
    "CSS" : 600
    "Other" : 200
```


## Regular code block (not mermaid)

This should render as a normal syntax-highlighted code block, not a diagram:

```swift
let html = Renderer.renderUp("# Hello\n")
```
