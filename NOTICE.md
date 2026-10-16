# Notice and provenance

Fabushi iOS is a standalone iOS application repository exported from
`bhrumom/fabushi`. The export provenance recorded by this repository is:

- source repository: `bhrumom/fabushi`
- source commit: `7851b689d2fe3fc3893cd9f4363899cc4a03e83b`
- exported boundary: `ios`
- original exported roots: `mobile/ios` and `mobile/native/include`

The Grok Bot 0.18 architecture parity work in this repository uses the pinned
research reconstruction
`b-nnett/grok-bot-0.18-reconstructed@a9f633e09d49a85829b8236331b9e21f7e612634`
as an architecture and behavioral reference. That reference repository states
that it is an unofficial reconstruction derived from a publicly distributed
binary application and that no upstream source-code license is asserted or
granted for the reconstructed material.

Accordingly:

1. the parity ledger is evidence of behavioral/module mapping, not evidence of
   an upstream redistribution license;
2. reconstructed names, contracts, or implementation material must not be
   represented as official upstream source;
3. original Grok Bot installers, extracted application payloads, signatures,
   or Anysphere credentials must not be added to this repository or its release
   artifacts;
4. third-party dependency licenses and notices remain governed by their
   respective upstream projects; and
5. public redistribution of any code whose rights depend on the reconstructed
   Grok Bot material requires an independent rights review before release.

See `docs/provenance/grok-bot-0.18-rights-review.md` for the release gate and
review record.
## SwiftMath

Fabushi iOS uses `mgriebling/SwiftMath` version 1.7.3 for native LaTeX math
typesetting in Agent transcript messages. The dependency is pinned by the iOS
XcodeGen project and is used locally on-device; it does not introduce a WebView
or remote math-rendering service.

MIT License

Copyright (c) 2023 Computer Inspirations

Permission is hereby granted, free of charge, to any person obtaining a copy
of this software and associated documentation files (the "Software"), to deal
in the Software without restriction, including without limitation the rights
to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
copies of the Software, and to permit persons to whom the Software is
furnished to do so, subject to the following conditions:

The above copyright notice and this permission notice shall be included in all
copies or substantial portions of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
SOFTWARE.

