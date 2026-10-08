/** SHA-256 of the leaf certificate DER, in the relay wire format. */
export function normalizeFingerprint(input: string): string | null {
  const hex = input
    .trim()
    .replace(/^sha256[/:]/i, "")
    .replace(/[:\s]/g, "")
    .toLowerCase()
  return /^[0-9a-f]{64}$/.test(hex) ? hex : null
}

export function formatFingerprint(input: string): string {
  return (
    normalizeFingerprint(input)?.match(/../g)?.join(":").toUpperCase() ?? ""
  )
}
