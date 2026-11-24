# Snapgent desktop design

## Direction
A compact native developer utility, not an analytics dashboard. The primary job is to start the local bridge, verify server readiness, and inspect activity. Match the existing extension's yellow/black identity without reproducing its decorative browser-specific styling.

## Tokens
| Token | Value | Purpose |
|---|---|---|
| Canvas | #161618 | Neutral desktop background |
| Surface | #1E1E20 | Controls and grouped content |
| Console | #0E0C06 | Activity log background |
| Border | #38383B | Quiet structural separation |
| Text | #ECE6CF | Primary content |
| Muted | #A8A8AE | Secondary readable content |
| Accent | #EAB308 | Primary action and selected state |
| Success | #5CBF9A | Confirmed healthy state |
| Error | #EF6B6B | Actual failure |

Source: snapgent-extension/overlay.css, particularly the yellow/black menu and tool-chip theme.

Typography: Windows Segoe UI for controls and labels where available; Cascadia Mono or the native monospace fallback for logs, ports and command data. No oversized technical headings.

Spacing: 4, 8, 12, 16, 24 points. Control radius 6, grouped panel radius 8. Use flat surfaces, no gradients, no glow, no decorative badges.

## Composition
- Product header with version and functional Overview, Activity, Settings navigation.
- Overview: bridge lifecycle and real status, then a compact MCP server list. Avoid repeated metric cards.
- Activity: searchable real log lines, copy, open log file and follow in terminal.
- Settings: extension ID, pairing credential and explicit session-level terminal authorization.
- Local approval: actual requested operation, deny and allow once. Escape denies.
- System tray: hide, restore and quit. No terminal window during normal operation.

## Design dials
Density: compact but legible. Rhythm: functional separation between control, list and log views. Motion: minimal, only interaction feedback. Decoration: none beyond meaningful product branding.

## Delivery checks
Apply Anti-Slop core and antislop-ui during implementation. Verify readable contrast, keyboard focus, resizing at minimum dimensions, real control behavior, empty/loading/error states and actual runtime data. Desktop-specific checks replace irrelevant website marketing and mobile navigation rules. Do not claim visual verification until the compiled window has been inspected.

Reference: https://github.com/miqdadbadjuber/anti-slop/tree/main/skills
