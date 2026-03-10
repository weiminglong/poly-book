## ADDED Requirements

### Requirement: useTheme hook unit tests
The `useTheme` hook SHALL have unit tests verifying theme toggling and persistence.

#### Scenario: Theme toggles between dark and light
- **WHEN** the toggle function returned by useTheme is called
- **THEN** the theme switches from dark to light or light to dark

#### Scenario: Theme persists across renders
- **WHEN** a theme is selected and the component re-renders
- **THEN** the hook returns the previously selected theme

### Requirement: useKeyboardShortcut hook unit tests
The `useKeyboardShortcut` hook SHALL have unit tests verifying shortcut registration
and callback invocation.

#### Scenario: Registered shortcut fires callback
- **WHEN** the registered key combination is pressed
- **THEN** the callback function is invoked

#### Scenario: Shortcut does not fire when input is focused
- **WHEN** the registered key is pressed while a text input has focus
- **THEN** the callback is not invoked

### Requirement: useOrderBookStream hook unit tests
The `useOrderBookStream` hook SHALL have unit tests verifying WebSocket connection
lifecycle and data updates.

#### Scenario: Hook returns initial loading state
- **WHEN** the hook mounts with an asset ID
- **THEN** it returns a loading state before data arrives

#### Scenario: Hook updates on WebSocket message
- **WHEN** a WebSocket message is received
- **THEN** the hook returns updated orderbook data

### Requirement: useSourceMode hook unit tests
The `useSourceMode` hook SHALL have unit tests verifying source mode toggling.

#### Scenario: Source mode toggles between live and demo
- **WHEN** the toggle function is called
- **THEN** the source mode switches between live and demo

### Requirement: useThrottledState hook unit tests
The `useThrottledState` hook SHALL have unit tests verifying throttle behavior.

#### Scenario: Rapid updates are throttled
- **WHEN** the state setter is called multiple times within the throttle window
- **THEN** only the last value is applied after the throttle period
