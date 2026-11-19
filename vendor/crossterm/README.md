# crossterm (vendored)

crossterm 0.29.0 with one change: an Application Program Command (`ESC _ ... ESC \`)
from the terminal is read as `Event::ApplicationProgramCommand` instead of as key
presses. viewmer needs it for kitty's graphics protocol replies.

Taken from https://github.com/monoamine11231/crossterm-apc at commit
289320dcb9e9f26967ec01864e6baf2d06b07ff9, the fork MeowPDF uses. That fork also derives
`Ord` on the event types and makes `document-features` optional. Examples and
dev-dependencies are left out, and one unused-parentheses warning is fixed. viewmer runs
on Unix only, so the Windows code goes, and so do the features it doesn't use (`serde`,
`osc52`, `event-stream`, `use-dev-tty`, `document-features`). MIT, see
[LICENSE](LICENSE).
