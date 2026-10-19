// Ad-hoc signatures have no Team ID and cannot satisfy hardened library validation.
// Keep hardened runtime for certificate-signed, notarizable distribution builds.
export function macosSigningConfig(identity) {
  if (!identity?.trim()) throw new Error('A macOS signing identity is required.');
  return { bundle: { macOS: { hardenedRuntime: identity.trim() !== '-' } } };
}
