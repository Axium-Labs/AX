# Browser and desktop architecture

The implemented architecture is documented in [host-permissions.md](host-permissions.md),
[computer-use.md](computer-use.md) and [ADR 0024](adr/0024-independent-host-access.md).

AX uses its existing ToolRegistry, scheduler, ApprovalPolicy and permission ceilings.
Files and terminal commands retain SandboxManager; native desktop and owned browser
operations use AuthorizedHost with independent exact app/site grants. Desktop uses
a disposable native Windows UIA worker. Browser uses the embedded Node/Playwright
worker, nonpersistent Chromium contexts, strict origin checks and inline images.
There is no Chromiumoxide implementation, arbitrary JavaScript/evaluate tool,
personal browser attachment or external Playwright CLI session access.

Platform support and functional limits are explicit in the canonical documents.
This separation does not provide a new Windows/macOS OS sandbox backend.
