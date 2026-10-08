import { describe, expect, it } from "vitest"
import { formatFingerprint, normalizeFingerprint } from "./fingerprint"

const hex = "0123456789abcdef".repeat(4)
const display = "01:23:45:67:89:AB:CD:EF:".repeat(4).slice(0, -1)

describe("relay fingerprints", () => {
  it.each([hex, display, ` sha256/${display} `, `SHA256: ${display}`])(
    "normalizes %s to 64 lowercase hex digits",
    (input) => expect(normalizeFingerprint(input)).toBe(hex)
  )

  it.each(["", "ab", "ab".repeat(33), "gh".repeat(32), `SHA1:${hex}`])(
    "rejects invalid input %s",
    (input) => expect(normalizeFingerprint(input)).toBeNull()
  )

  it("formats canonical hex for human verification", () => {
    expect(formatFingerprint(hex)).toBe(display)
    expect(formatFingerprint("invalid")).toBe("")
  })
})
