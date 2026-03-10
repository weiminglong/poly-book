## ADDED Requirements

### Requirement: Design token system
The application SHALL define a design token system using CSS custom properties, configured via Tailwind CSS v4's `@theme` directive. Tokens SHALL cover: color palette (background, foreground, muted, accent, destructive, warning, success), spacing scale (4px base), font sizes (5 steps), border radii (3 steps), and shadow levels (3 steps). All components SHALL reference tokens, never raw hex/rgb values.

#### Scenario: Theme consistency
- **WHEN** a developer creates a new component
- **THEN** the component uses Tailwind utility classes that reference design tokens (e.g., `bg-background`, `text-muted`, `rounded-md`) rather than arbitrary values

#### Scenario: Token override
- **WHEN** a deployment needs a custom accent color
- **THEN** overriding the `--color-accent` CSS custom property at the root element changes the accent color across all components

### Requirement: Dark and light themes
The application SHALL support dark (default) and light themes. Theme selection SHALL be persisted in `localStorage` and default to `dark`. Theme switching SHALL be instant (no flash of unstyled content) and SHALL use CSS custom properties so that no JavaScript re-render is needed for theme changes.

#### Scenario: Theme toggle
- **WHEN** the user clicks the theme toggle in the app header
- **THEN** all colors update instantly to the selected theme via CSS custom property changes
- **AND** the preference is persisted to `localStorage`

#### Scenario: Page reload preserves theme
- **WHEN** the user reloads the page after selecting light theme
- **THEN** the app loads in light theme without a flash of dark theme

### Requirement: Density modes
The application SHALL support three density modes: compact, comfortable (default), and spacious. Density mode SHALL affect padding, gap, font-size, and row-height tokens globally. Density preference SHALL be persisted in `localStorage`.

#### Scenario: Compact mode for data-dense views
- **WHEN** the user selects compact mode
- **THEN** table rows, card padding, and metric card sizes shrink to fit more data on screen
- **AND** all text remains legible (minimum 11px rendered font size)

#### Scenario: Spacious mode for presentation
- **WHEN** the user selects spacious mode
- **THEN** padding and gaps increase for a more relaxed layout suitable for screen sharing

### Requirement: Shared component library
The application SHALL provide a shared component library in `shared/components/` including at minimum: Card, MetricCard, Badge (replacing `.pill`), Table, DataTable (sortable/paginated), ErrorBanner, Skeleton, Tooltip, Button, Input, Select, Dialog, and CommandPalette. Each component SHALL be a self-contained file using Tailwind classes and Radix UI primitives where applicable.

#### Scenario: Card component
- **WHEN** a feature needs a content container
- **THEN** it imports `Card` from `shared/components/card` which provides consistent border, background, padding, and optional header/toolbar slots

#### Scenario: DataTable with sorting
- **WHEN** a feature needs a sortable table (e.g., execution events)
- **THEN** it uses `DataTable` from `shared/components/data-table` which accepts column definitions and row data, and supports click-to-sort on column headers

#### Scenario: Badge variants
- **WHEN** a feature needs a status indicator
- **THEN** it uses `Badge` with a `variant` prop (`success`, `warning`, `error`, `neutral`) that maps to design token colors

### Requirement: Accessible interactive components
All interactive components (buttons, form controls, dialogs, tooltips, select menus) SHALL meet WCAG 2.1 AA compliance. Components using overlays (Dialog, Tooltip, CommandPalette) SHALL use Radix UI primitives for correct focus trapping, keyboard navigation, and screen reader announcements.

#### Scenario: Dialog focus management
- **WHEN** a dialog opens
- **THEN** focus moves to the first focusable element inside the dialog
- **AND** pressing Escape closes the dialog and returns focus to the trigger element

#### Scenario: Keyboard-navigable select
- **WHEN** a user focuses a Select component and presses Arrow Down
- **THEN** the dropdown opens and the first option is highlighted
- **AND** pressing Enter selects the highlighted option and closes the dropdown

### Requirement: Motion and transitions
Interactive elements (buttons, cards, navigation links) SHALL have subtle transition effects (border-color, background-color, transform) with duration of 100-200ms. The application SHALL respect `prefers-reduced-motion` by disabling transitions when the user's OS setting requests it.

#### Scenario: Reduced motion preference
- **WHEN** the user has `prefers-reduced-motion: reduce` enabled in their OS
- **THEN** all CSS transitions and animations are disabled or reduced to instant state changes

#### Scenario: Button hover feedback
- **WHEN** the user hovers over a primary button
- **THEN** the button's background color transitions smoothly over 150ms
