# Security policy

## Supported versions

Only the latest release gets fixes.

## Reporting a vulnerability

Please report it privately through GitHub: open the repository's "Security" tab and choose
"Report a vulnerability". Don't open a public issue. Say what an attacker can do, how to
reproduce it, and which version you tested. Expect a first reply within a week.

## Scope

In scope is this server's own code, for example:

- the disk cache (paths, temp files, what a crafted answer can make it write);
- validation of tool arguments and of answers from the data sources (parsing, size limits);
- the cleanup of text written by people, such as Steam names and wiki pages, which strips control,
  zero-width and bidirectional characters before the text reaches the model;
- a vulnerable dependency that the server actually uses.

The Deadlock API and the Deadlock Wiki are separate projects; report problems with them to their
maintainers. The text of wiki pages and Steam names is untrusted by nature, and the tools are marked
as reading an open world for that reason. A way to get hidden instructions past the cleanup above is
in scope.
