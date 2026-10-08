import { act, fireEvent, render, screen, waitFor } from "@testing-library/react"
import { NextIntlClientProvider } from "next-intl"
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest"
import en from "@/i18n/messages/en.json"
import type { RelayConfig, RelayStatus } from "@/lib/types"

vi.mock("@/lib/api", () => ({
  getRelayConfig: vi.fn(),
  updateRelayConfig: vi.fn(),
  getRelayStatus: vi.fn(),
  testRelayConnection: vi.fn(),
}))
import {
  getRelayConfig,
  getRelayStatus,
  testRelayConnection,
  updateRelayConfig,
} from "@/lib/api"
import { WebServiceRelaySection } from "./web-service-relay-section"

const config: RelayConfig = {
  enabled: false,
  url: "https://relay.test:18443",
  key_set: true,
  fingerprint: null,
  public_url: null,
  effective_public_url: "https://relay.test:18443",
  source: "settings",
}
const status: RelayStatus = {
  state: "disabled",
  url: null,
  connected_since: null,
  last_rtt_ms: null,
  active_streams: 0,
  total_streams: 0,
  last_error: null,
  next_retry_in_ms: null,
  relay_cert_fingerprint: null,
}
const fingerprint = "ab".repeat(32)
const presented = "AB:".repeat(32).slice(0, -1)
const renderSection = () =>
  render(
    <NextIntlClientProvider locale="en" messages={en}>
      <WebServiceRelaySection />
    </NextIntlClientProvider>
  )

beforeEach(() => {
  vi.resetAllMocks()
  Object.defineProperty(document, "visibilityState", {
    configurable: true,
    value: "visible",
  })
  vi.mocked(getRelayConfig).mockResolvedValue(config)
  vi.mocked(getRelayStatus).mockResolvedValue(status)
  vi.mocked(updateRelayConfig).mockResolvedValue(config)
})
afterEach(() => vi.useRealTimers())

describe("remote relay settings", () => {
  it("loads config without exposing a stored key and saves flat normalized fields", async () => {
    renderSection()
    const url = await screen.findByLabelText("Relay URL")
    expect(url).toHaveValue(config.url)
    expect(screen.getByText("Set")).toBeInTheDocument()
    expect(screen.getByLabelText("Relay key")).toHaveValue("")
    fireEvent.click(screen.getByRole("switch", { name: "Enable relay" }))
    fireEvent.change(url, { target: { value: "https://new.test:18443" } })
    fireEvent.change(
      screen.getByLabelText("Certificate fingerprint (optional)"),
      { target: { value: `SHA256: ${presented}` } }
    )
    fireEvent.click(screen.getByRole("button", { name: "Save" }))
    await waitFor(() =>
      expect(updateRelayConfig).toHaveBeenCalledWith({
        enabled: true,
        url: "https://new.test:18443",
        fingerprint,
        publicUrl: null,
      })
    )
    expect(await screen.findByText("Relay settings saved")).toBeInTheDocument()
  })

  it("replaces the key only when explicitly edited, with password show/hide", async () => {
    renderSection()
    fireEvent.click(await screen.findByRole("button", { name: "Replace" }))
    const input = screen.getByLabelText("Relay key")
    expect(input).toHaveAttribute("type", "password")
    fireEvent.change(input, { target: { value: "new-test-key-12345678" } })
    fireEvent.click(screen.getByRole("button", { name: "Show" }))
    expect(input).toHaveAttribute("type", "text")
    fireEvent.click(screen.getByRole("button", { name: "Save" }))
    await waitFor(() =>
      expect(updateRelayConfig).toHaveBeenCalledWith(
        expect.objectContaining({ key: "new-test-key-12345678" })
      )
    )
    await waitFor(() => expect(input).toHaveValue(""))
  })

  it("shows inline backend validation errors", async () => {
    vi.mocked(updateRelayConfig).mockRejectedValue({
      code: "invalid_relay_url",
      message: "HTTPS is required",
    })
    renderSection()
    fireEvent.click(await screen.findByRole("button", { name: "Save" }))
    expect(await screen.findByRole("alert")).toHaveTextContent(
      "HTTPS is required"
    )
  })

  it("requires explicit trust before saving the presented certificate", async () => {
    vi.mocked(testRelayConnection).mockResolvedValue({
      ok: false,
      fingerprint: presented,
      rtt_ms: null,
      error: "Untrusted certificate",
      error_code: "tls_untrusted",
    })
    renderSection()
    fireEvent.click(
      await screen.findByRole("button", { name: "Test connection" })
    )
    const trust = await screen.findByRole("button", {
      name: "Trust this certificate",
    })
    expect(screen.getByText(presented)).toBeInTheDocument()
    expect(updateRelayConfig).not.toHaveBeenCalled()
    expect(testRelayConnection).toHaveBeenCalledWith({
      url: config.url,
      fingerprint: null,
    })
    fireEvent.click(trust)
    await waitFor(() =>
      expect(updateRelayConfig).toHaveBeenCalledWith({
        enabled: false,
        url: config.url,
        fingerprint,
        publicUrl: null,
      })
    )
  })

  it("discards pending trust when the tested URL changes", async () => {
    vi.mocked(testRelayConnection).mockResolvedValue({
      ok: false,
      fingerprint,
      rtt_ms: null,
      error: null,
      error_code: "tls_untrusted",
    })
    renderSection()
    fireEvent.click(
      await screen.findByRole("button", { name: "Test connection" })
    )
    await screen.findByRole("button", { name: "Trust this certificate" })
    fireEvent.change(screen.getByLabelText("Relay URL"), {
      target: { value: "https://other.test" },
    })
    expect(
      screen.queryByRole("button", { name: "Trust this certificate" })
    ).not.toBeInTheDocument()
    expect(updateRelayConfig).not.toHaveBeenCalled()
  })

  it("warns prominently on fingerprint mismatch without offering TOFU", async () => {
    vi.mocked(testRelayConnection).mockResolvedValue({
      ok: false,
      fingerprint,
      rtt_ms: null,
      error: "Mismatch",
      error_code: "fingerprint_mismatch",
    })
    renderSection()
    fireEvent.click(
      await screen.findByRole("button", { name: "Test connection" })
    )
    expect(await screen.findByRole("alert")).toHaveTextContent(
      "possible interception"
    )
    expect(
      screen.queryByRole("button", { name: "Trust this certificate" })
    ).not.toBeInTheDocument()
    expect(updateRelayConfig).not.toHaveBeenCalled()
  })

  it("makes environment configuration read-only", async () => {
    vi.mocked(getRelayConfig).mockResolvedValue({ ...config, source: "env" })
    renderSection()
    expect(await screen.findByLabelText("Relay URL")).toHaveAttribute(
      "readonly"
    )
    expect(
      screen.getByLabelText("Certificate fingerprint (optional)")
    ).toHaveAttribute("readonly")
    expect(screen.getByLabelText("Public URL override")).toHaveAttribute(
      "readonly"
    )
    expect(screen.getByRole("switch")).toBeDisabled()
    expect(screen.getByRole("button", { name: "Save" })).toBeDisabled()
    expect(
      screen.queryByRole("button", { name: "Replace" })
    ).not.toBeInTheDocument()
  })

  it.each([
    "Command get_relay_config not found",
    { code: "not_implemented", message: "Pending" },
  ])("degrades gracefully with missing backend %j", async (error) => {
    vi.mocked(getRelayConfig).mockRejectedValue(error)
    renderSection()
    expect(
      await screen.findByText("Relay support unavailable in this build")
    ).toBeInTheDocument()
    expect(
      screen.queryByRole("button", { name: "Save" })
    ).not.toBeInTheDocument()
  })

  it("does not save default values after a failed read and allows retry", async () => {
    vi.mocked(getRelayConfig).mockRejectedValueOnce(new Error("Offline"))
    renderSection()
    fireEvent.click(await screen.findByRole("button", { name: "Retry" }))
    expect(await screen.findByLabelText("Relay URL")).toHaveValue(config.url)
    expect(updateRelayConfig).not.toHaveBeenCalled()
  })

  it("rejects invalid fingerprints before calling the backend", async () => {
    renderSection()
    fireEvent.change(
      await screen.findByLabelText("Certificate fingerprint (optional)"),
      {
        target: { value: "not-a-fingerprint" },
      }
    )
    fireEvent.click(screen.getByRole("button", { name: "Save" }))
    expect(await screen.findByRole("alert")).toHaveTextContent(
      "64 hexadecimal digits"
    )
    expect(updateRelayConfig).not.toHaveBeenCalled()
  })

  it("does not offer a trust write for environment-managed configuration", async () => {
    vi.mocked(getRelayConfig).mockResolvedValue({ ...config, source: "env" })
    vi.mocked(testRelayConnection).mockResolvedValue({
      ok: false,
      fingerprint,
      rtt_ms: null,
      error: null,
      error_code: "tls_untrusted",
    })
    renderSection()
    fireEvent.click(
      await screen.findByRole("button", { name: "Test connection" })
    )
    await screen.findByRole("alert")
    expect(
      screen.queryByRole("button", { name: "Trust this certificate" })
    ).not.toBeInTheDocument()
    expect(updateRelayConfig).not.toHaveBeenCalled()
  })

  it.each([
    ["disabled", "Disabled"],
    ["waiting_for_web_service", "Waiting for web service"],
    ["connecting", "Connecting"],
    ["connected", "Connected · RTT 42 ms · streams 3"],
    ["backoff", "Retrying in 2 s"],
    ["error", "Error: Offline"],
  ] as const)(
    "renders the %s status with its backend metrics",
    async (state, label) => {
      vi.mocked(getRelayStatus).mockResolvedValue({
        ...status,
        state,
        last_rtt_ms: 42,
        active_streams: 3,
        next_retry_in_ms: 1500,
        last_error: "Offline",
      })
      renderSection()
      expect(await screen.findByText(label)).toBeInTheDocument()
    }
  )

  it("degrades if the status command is missing after config loaded", async () => {
    vi.mocked(getRelayStatus).mockRejectedValue({ code: "not_implemented" })
    renderSection()
    expect(
      await screen.findByText("Relay support unavailable in this build")
    ).toBeInTheDocument()
    expect(
      screen.queryByRole("button", { name: "Save" })
    ).not.toBeInTheDocument()
  })

  it("does not poll while initially hidden or overlap a slow status request", async () => {
    vi.useFakeTimers()
    Object.defineProperty(document, "visibilityState", {
      configurable: true,
      value: "hidden",
    })
    vi.mocked(getRelayStatus).mockReturnValue(new Promise(() => {}))
    const view = renderSection()
    await act(async () => {})
    await act(() => vi.advanceTimersByTimeAsync(4000))
    expect(getRelayStatus).not.toHaveBeenCalled()
    Object.defineProperty(document, "visibilityState", {
      configurable: true,
      value: "visible",
    })
    await act(async () => document.dispatchEvent(new Event("visibilitychange")))
    await act(() => vi.advanceTimersByTimeAsync(6000))
    expect(getRelayStatus).toHaveBeenCalledTimes(1)
    view.unmount()
  })

  it("polls every two seconds only while visible and cleans up on unmount", async () => {
    vi.useFakeTimers()
    const view = renderSection()
    await act(async () => {})
    expect(getRelayStatus).toHaveBeenCalledTimes(1)
    await act(() => vi.advanceTimersByTimeAsync(2000))
    expect(getRelayStatus).toHaveBeenCalledTimes(2)
    Object.defineProperty(document, "visibilityState", {
      configurable: true,
      value: "hidden",
    })
    act(() => document.dispatchEvent(new Event("visibilitychange")))
    await act(() => vi.advanceTimersByTimeAsync(6000))
    expect(getRelayStatus).toHaveBeenCalledTimes(2)
    Object.defineProperty(document, "visibilityState", {
      configurable: true,
      value: "visible",
    })
    await act(async () => document.dispatchEvent(new Event("visibilitychange")))
    expect(getRelayStatus).toHaveBeenCalledTimes(3)
    view.unmount()
    await act(() => vi.advanceTimersByTimeAsync(4000))
    expect(getRelayStatus).toHaveBeenCalledTimes(3)
  })
})
