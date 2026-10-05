# Vendored script libraries

Embedded into the binary and evaluated lazily when a script calls `require(...)`.

| Module | Version | File | License |
|---|---|---|---|
| chai | 4.5.0 | chai.js | MIT |
| lodash | 4.17.21 | lodash.min.js | MIT |
| crypto-js | 4.2.0 | crypto-js.js | MIT |
| moment | 2.30.1 | moment.min.js | MIT |
| tv4 | 1.3.0 | tv4.js | Public domain / MIT |
| ajv | 8.17.1 | ajv.bundle.js (esbuild IIFE, global `__ajv`) | MIT |

Regenerate ajv: `esbuild ajv-entry.js --bundle --minify --format=iife --global-name=__ajv --platform=neutral`.
