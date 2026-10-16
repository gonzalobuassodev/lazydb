# Linux runtime compatibility

Linux release archives include a runtime report for each architecture. The report is derived from the ELF executable in the final archive and is bound to both the archive and executable SHA-256 digests. It records the ELF interpreter, shared libraries, GLIBC version nodes, weak references, and the minimum non-weak glibc version.

The channel schema 2 manifest publishes each Linux target's required glibc family and minimum version. New shell installers and native updaters request schema 2 first. They use the legacy schema 1 manifest only when the schema 2 URL returns HTTP 404. Network failures, other HTTP errors, malformed requirements, and known incompatible glibc versions stop before the archive is downloaded. Activation still requires the staged executable to pass its version check.

Check the installed glibc version with `getconf GNU_LIBC_VERSION`. The published minimum describes the analyzed release artifact; it is not a claim that every Linux distribution at that numeric version has been tested. Native release and runtime evidence is recorded per architecture in CI. Optional components such as database drivers, keyring services, fonts, and Oracle Instant Client are not Linux runtime requirements unless the executable directly requires their native libraries.

Release policy changes must update the architecture policy, preserve the ELF evidence, and pass native build, archive, package, and runtime checks. See [releasing](releasing.md) for the release workflow.
