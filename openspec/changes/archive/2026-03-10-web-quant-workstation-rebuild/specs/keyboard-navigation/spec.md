## ADDED Requirements

### Requirement: Command palette
The application SHALL provide a command palette activated by Cmd+K (Mac) or Ctrl+K (Windows/Linux). The palette SHALL allow searching and navigating to any page, toggling theme, switching density mode, and switching data source (api/demo). The palette SHALL use fuzzy matching on command names.

#### Scenario: Open and navigate
- **WHEN** the user presses Cmd+K and types "replay"
- **THEN** the command palette shows "Replay Workbench" as a matching result
- **AND** pressing Enter navigates to the Replay Workbench page and closes the palette

#### Scenario: Toggle theme via palette
- **WHEN** the user opens the command palette and types "theme"
- **THEN** "Toggle theme (dark/light)" appears as a matching command
- **AND** selecting it toggles the theme immediately

#### Scenario: Dismiss palette
- **WHEN** the command palette is open and the user presses Escape
- **THEN** the palette closes and focus returns to the previously focused element

#### Scenario: No results
- **WHEN** the user types a query that matches no commands
- **THEN** the palette shows "No results found" in muted text

### Requirement: Page-level keyboard shortcuts
Each page SHALL support keyboard shortcuts for common actions. Shortcuts SHALL NOT fire when the user is typing in an input, select, or textarea element. A help overlay (activated by `?`) SHALL list all available shortcuts for the current page.

#### Scenario: Orderbook depth shortcut
- **WHEN** the user presses `1` through `6` on the orderbook page (not in an input)
- **THEN** the depth changes to the preset value for that key (1=5, 2=10, 3=25, 4=50, 5=100, 6=200)

#### Scenario: Refresh shortcut
- **WHEN** the user presses `r` on any page (not in an input)
- **THEN** the page's primary data query is invalidated and refetched

#### Scenario: Help overlay
- **WHEN** the user presses `?` on any page (not in an input)
- **THEN** a modal overlay appears listing all keyboard shortcuts for the current page with their descriptions

#### Scenario: Input field passthrough
- **WHEN** the user is focused on a text input and presses `r`
- **THEN** the character `r` is typed into the input and no keyboard shortcut is triggered

### Requirement: Focus management
The application SHALL manage focus predictably across page transitions and modal interactions. When navigating to a new page, focus SHALL move to the page's main heading. When a modal closes, focus SHALL return to the element that triggered the modal.

#### Scenario: Page navigation focus
- **WHEN** the user navigates from Live Feed to Orderbook via the nav bar
- **THEN** after the page loads, focus moves to the Orderbook page's main heading (or first interactive element)

#### Scenario: Modal focus return
- **WHEN** the user opens the command palette from the nav bar and closes it
- **THEN** focus returns to the nav element that was focused before the palette opened
