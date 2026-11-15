#!/bin/bash
# (temporary) the suites page streaming touches: live (all), the CLI's watch and stream tests and
# its bins' (ui), tex's pdfout and xdvpdf units, the pdf crate's.
set -u
cargo test -q --profile fastdev -p phitex-live --lib 2>&1 | grep -E "FAILED|panicked|test result" | head -20
cargo test -q --profile fastdev -p phitex-cli --test cli -- watch_ stream 2>&1 | grep -E "FAILED|panicked|test result" | head -20
cargo test -q --profile fastdev -p phitex-cli --bins 2>&1 | grep -E "FAILED|panicked|test result" | head
cargo test -q --profile fastdev -p phitex-tex --lib -- pdfout xdvpdf exec 2>&1 | grep -E "FAILED|panicked|test result" | head
cargo test -q --profile fastdev -p phitex-pdf 2>&1 | grep -E "FAILED|panicked|test result" | head
