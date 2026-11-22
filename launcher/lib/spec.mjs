// What to run and from where. The launcher's version decides the Python
// version, so the two cannot drift apart.
import { readFileSync } from 'node:fs';

const pkg = JSON.parse(readFileSync(new URL('../../package.json', import.meta.url), 'utf8'));
export const VERSION = pkg.version;
export const REPOSITORY = 'adeelahmad/ankiplusMCP';
// Releases are published as branches named release-v<version> (git tags can't be pushed
// from the build environment). A release branch only ever receives fixes for that version.
export const RELEASE = `release-v${VERSION}`;

/** The Python requirement uv installs. ANKIPLUSMCP_PY_SPEC overrides it for development. */
export function pySpec(env) {
  return env.ANKIPLUSMCP_PY_SPEC || `ankiplusmcp[all] @ git+https://github.com/${REPOSITORY}@${RELEASE}`;
}

/** What host configs pass to `npx -y`. Becomes `ankiplusmcp@<version>` once published to npm. */
export function npxRef(env) {
  return env.ANKIPLUSMCP_NPX_REF || `github:${REPOSITORY}#${RELEASE}`;
}
