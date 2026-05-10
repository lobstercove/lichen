'use strict';

const fs = require('fs');

function requirePrivateDocs(label, paths) {
  const missing = paths.filter((filePath) => !fs.existsSync(filePath));
  if (missing.length === 0) {
    return;
  }

  process.stdout.write(`\n${label}: skipped; internal docs are not present in this checkout.\n`);
  process.exit(0);
}

module.exports = { requirePrivateDocs };
