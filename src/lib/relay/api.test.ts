import { beforeEach, expect, it, vi } from "vitest"
const { call } = vi.hoisted(() => ({ call: vi.fn() }))
vi.mock("@/lib/transport", () => ({ getTransport: () => ({ call }) }))
import {
  getRelayConfig,
  getRelayStatus,
  testRelayConnection,
  updateRelayConfig,
} from "@/lib/api"

beforeEach(() => vi.clearAllMocks())

it("uses named config arguments and preserves omitted versus null fields", async () => {
  await getRelayConfig()
  expect(call).toHaveBeenLastCalledWith("get_relay_config")
  await getRelayStatus()
  expect(call).toHaveBeenLastCalledWith("get_relay_status")
  await updateRelayConfig({
    enabled: true,
    url: "https://relay.test",
    key: "test-key-12345678",
    fingerprint: null,
    publicUrl: null,
  })
  expect(call).toHaveBeenLastCalledWith("update_relay_config", {
    config: {
      enabled: true,
      url: "https://relay.test",
      key: "test-key-12345678",
      fingerprint: null,
      publicUrl: null,
    },
  })
  await updateRelayConfig({ enabled: false })
  expect(call).toHaveBeenLastCalledWith("update_relay_config", {
    config: { enabled: false },
  })
  await testRelayConnection({ url: "https://relay.test" })
  expect(call).toHaveBeenLastCalledWith("test_relay_connection", {
    config: { url: "https://relay.test" },
  })
})
