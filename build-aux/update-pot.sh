#!/bin/sh
# Regenerate po/flowlin.pot from the sources listed in po/POTFILES.
# Rust sources are scanned with the C lexer; strings are marked with
# tr("…") and trn("…", "…", n).
set -eu
cd "$(dirname "$0")/.."
grep -v '^#' po/POTFILES | grep '\.rs$' > /tmp/flowlin-potfiles-rs
xgettext --from-code=UTF-8 -L C --keyword=tr --keyword=trn:1,2 \
  --package-name=flowlin -f /tmp/flowlin-potfiles-rs -o po/flowlin.pot
for f in $(grep -v '^#' po/POTFILES | grep -v '\.rs$'); do
  xgettext --from-code=UTF-8 -j -o po/flowlin.pot "$f" || true
done
echo "wrote po/flowlin.pot ($(grep -c '^msgid' po/flowlin.pot) messages)"
# Build catalogues: for l in $(grep -v '^#' po/LINGUAS); do msgfmt po/$l.po -o locale/$l/LC_MESSAGES/flowlin.mo; done
