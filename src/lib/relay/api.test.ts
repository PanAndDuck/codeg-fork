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

it("uses exact relay command names and flat camelCase arguments", async () => {
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
    enabled: true,
    url: "https://relay.test",
    key: "test-key-12345678",
    fingerprint: null,
    publicUrl: null,
  })
  await updateRelayConfig({ enabled: false })
  expect(call).toHaveBeenLastCalledWith("update_relay_config", {
    enabled: false,
  })
  await testRelayConnection({ url: "https://relay.test" })
  expect(call).toHaveBeenLastCalledWith("test_relay_connection", {
    url: "https://relay.test",
  })
})
