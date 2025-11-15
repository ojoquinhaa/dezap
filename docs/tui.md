# Text User Interface

## Layout

- **Left column** is dedicated to the chat stream and message input. Messages are rendered with timestamps, color-coded direction badges, and a scrollable list widget.
- **Right column** contains the header (Dezap banner + ASCII demon), status summary (handle, IP, discovery state), transfer progress gauges, discovery/peer panels, saved peers snapshot, and help table.
- Input area supports multi-line typing with automatic wrapping. Cursor is hidden while browsing chat history to avoid confusion.

## Interaction

- Navigation is predominantly keyboard-controlled. `Enter` commits the input, while `Ctrl+F`, `Ctrl+L`, `Ctrl+K`, `Ctrl+D`, `Ctrl+U`, `Ctrl+R`, and `Ctrl+X` trigger file send, listening, connect, discovery, rename, discovery network change, and disconnected states respectively.
- `Ctrl+P` focuses the discovered peers list and `Ctrl+S` the saved peers list. Use `↑/↓` to highlight an entry and `Enter` to connect, Esc to cancel.
- `Ctrl+G` toggles chat-browse mode. While browsing, `↑/↓` move through past messages, `c` copies the highlighted entry to the clipboard, and `Esc` leaves the focus.
- Incoming file offers pre-fill the download path; edit the line and press `Enter` to accept or `Esc` to decline. Offers queue until handled.
- When file send mode is active, `Tab` performs filesystem autocomplete, suggesting directories and showing a short preview of candidate entries beneath the input.
