import { describe, expect, it } from "vitest"
import { buildPairingPayload } from "./pairing"

describe("mobile pairing contract", () => {
  it("encodes a relay payload in the exact v1 shape", () => {
    expect(
      buildPairingPayload({
        mode: "relay",
        url: "https://relay.test:18443",
        token: "test-token",
        fp: "ab".repeat(32),
        name: "Home",
      })
    ).toBe(
      '{"v":1,"mode":"relay","url":"https://relay.test:18443","token":"test-token","fp":"abababababababababababababababababababababababababababababababab","name":"Home"}'
    )
  })

  it("omits optional fields from a direct LAN payload", () => {
    expect(
      buildPairingPayload({
        mode: "direct",
        url: "http://192.0.2.1:3090",
        token: 'test"token',
      })
    ).toBe(
      '{"v":1,"mode":"direct","url":"http://192.0.2.1:3090","token":"test\\"token"}'
    )
  })
})
