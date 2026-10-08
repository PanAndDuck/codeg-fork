import { act, fireEvent, render, screen, waitFor } from "@testing-library/react"
import { NextIntlClientProvider } from "next-intl"
import { toast } from "sonner"
import { afterEach, beforeEach, expect, it, vi } from "vitest"
import en from "@/i18n/messages/en.json"
import type { RelayConfig } from "@/lib/types"
import { AddressQrcodeDialog } from "./web-service-settings"

vi.mock("@/lib/api", () => ({
  getRelayConfig: vi.fn(),
  getRelayStatus: vi.fn(),
  updateRelayConfig: vi.fn(),
  testRelayConnection: vi.fn(),
  startWebServer: vi.fn(),
  stopWebServer: vi.fn(),
  getWebServerStatus: vi.fn(),
  getWebServiceConfig: vi.fn(),
  updateWebServiceConfig: vi.fn(),
  probeWebServicePort: vi.fn(),
}))
vi.mock("qrcode.react", () => ({
  QRCodeSVG: ({ value }: { value: string }) => (
    <output data-testid="qr-value">{value}</output>
  ),
}))
vi.mock("sonner", () => ({ toast: { success: vi.fn() } }))

const writeText = vi.fn().mockResolvedValue(undefined)
const originalClipboard = Object.getOwnPropertyDescriptor(
  navigator,
  "clipboard"
)

const relay: RelayConfig = {
  enabled: true,
  url: "https://relay.test:18443",
  key_set: true,
  fingerprint: "ab".repeat(32),
  public_url: null,
  effective_public_url: "https://relay.test:18443",
  source: "settings",
}
const props = {
  open: true,
  address: "http://192.0.2.1:3090",
  token: "test-web-token",
  name: "Home",
  relayConfig: relay,
  onOpenChange: vi.fn(),
}
function dialog(next = props) {
  return (
    <NextIntlClientProvider locale="en" messages={en}>
      <AddressQrcodeDialog {...next} />
    </NextIntlClientProvider>
  )
}
beforeEach(() => {
  vi.clearAllMocks()
  writeText.mockReset().mockResolvedValue(undefined)
  Object.defineProperty(navigator, "clipboard", {
    configurable: true,
    value: { writeText },
  })
})
afterEach(() => {
  if (originalClipboard) {
    Object.defineProperty(navigator, "clipboard", originalClipboard)
  } else {
    Reflect.deleteProperty(navigator, "clipboard")
  }
})

it.each(["Relay", "LAN + token"])(
  "only offers copying %s pairing codes after reveal, and removes it when hidden",
  (mode) => {
    render(dialog())
    fireEvent.click(screen.getByRole("button", { name: mode }))
    expect(
      screen.queryByRole("button", { name: "Copy pairing code" })
    ).not.toBeInTheDocument()

    fireEvent.click(
      screen.getByRole("button", { name: "Show pairing QR (contains token)" })
    )
    expect(
      screen.getByRole("button", { name: "Copy pairing code" })
    ).toBeEnabled()

    fireEvent.click(screen.getByRole("button", { name: "Hide pairing QR" }))
    expect(
      screen.queryByRole("button", { name: "Copy pairing code" })
    ).not.toBeInTheDocument()
    expect(writeText).not.toHaveBeenCalled()
  }
)

it.each([
  [
    "Relay",
    '{"v":1,"mode":"relay","url":"https://relay.test:18443","token":"test-web-token","fp":"abababababababababababababababababababababababababababababababab","name":"Home"}',
  ],
  [
    "LAN + token",
    '{"v":1,"mode":"direct","url":"http://192.0.2.1:3090","token":"test-web-token","name":"Home"}',
  ],
])(
  "copies the exact %s QR payload and warns about the token",
  async (mode, payload) => {
    render(dialog())
    fireEvent.click(screen.getByRole("button", { name: mode }))
    fireEvent.click(
      screen.getByRole("button", { name: "Show pairing QR (contains token)" })
    )
    expect(screen.getByTestId("qr-value").textContent).toBe(payload)
    expect(writeText).not.toHaveBeenCalled()
    expect(toast.success).not.toHaveBeenCalled()

    fireEvent.click(screen.getByRole("button", { name: "Copy pairing code" }))
    await waitFor(() => {
      expect(writeText).toHaveBeenCalledOnce()
      expect(writeText).toHaveBeenCalledWith(payload)
      expect(toast.success).toHaveBeenCalledOnce()
      expect(toast.success).toHaveBeenCalledWith(
        "Copied — contains your access token"
      )
    })
  }
)

it("does not report success when the clipboard write fails", async () => {
  writeText.mockRejectedValueOnce(new Error("Clipboard unavailable"))
  render(dialog())
  fireEvent.click(screen.getByRole("button", { name: "LAN + token" }))
  fireEvent.click(
    screen.getByRole("button", { name: "Show pairing QR (contains token)" })
  )
  await act(async () => {
    fireEvent.click(screen.getByRole("button", { name: "Copy pairing code" }))
  })
  expect(writeText).toHaveBeenCalledOnce()
  expect(toast.success).not.toHaveBeenCalled()
})

it("keeps LAN URL QR by default and gates both token payloads behind explicit clicks", async () => {
  render(dialog())
  expect(screen.getByTestId("qr-value")).toHaveTextContent(props.address)
  expect(
    screen.queryByRole("button", { name: "Copy pairing code" })
  ).not.toBeInTheDocument()
  fireEvent.click(screen.getByRole("button", { name: "Relay" }))
  expect(screen.queryByTestId("qr-value")).not.toBeInTheDocument()
  fireEvent.click(
    screen.getByRole("button", { name: "Show pairing QR (contains token)" })
  )
  expect(screen.getByTestId("qr-value").textContent).toBe(
    '{"v":1,"mode":"relay","url":"https://relay.test:18443","token":"test-web-token","fp":"abababababababababababababababababababababababababababababababab","name":"Home"}'
  )
  fireEvent.click(screen.getByRole("button", { name: "LAN" }))
  expect(screen.getByTestId("qr-value")).toHaveTextContent(props.address)
  expect(
    screen.queryByRole("button", { name: "Copy pairing code" })
  ).not.toBeInTheDocument()
  fireEvent.click(screen.getByRole("button", { name: "LAN + token" }))
  expect(screen.queryByTestId("qr-value")).not.toBeInTheDocument()
  fireEvent.click(
    screen.getByRole("button", { name: "Show pairing QR (contains token)" })
  )
  expect(screen.getByTestId("qr-value").textContent).toBe(
    '{"v":1,"mode":"direct","url":"http://192.0.2.1:3090","token":"test-web-token","name":"Home"}'
  )
})

it.each(["changed-token", ""])(
  "hides token QR and copying on close/reopen and when token changes to %j",
  async (token) => {
    const view = render(dialog())
    fireEvent.click(screen.getByRole("button", { name: "LAN + token" }))
    fireEvent.click(
      screen.getByRole("button", { name: "Show pairing QR (contains token)" })
    )
    view.rerender(dialog({ ...props, token }))
    await waitFor(() =>
      expect(screen.queryByTestId("qr-value")).not.toBeInTheDocument()
    )
    expect(
      screen.queryByRole("button", { name: "Copy pairing code" })
    ).not.toBeInTheDocument()
    view.rerender(dialog())
    expect(screen.queryByTestId("qr-value")).not.toBeInTheDocument()
    expect(
      screen.queryByRole("button", { name: "Copy pairing code" })
    ).not.toBeInTheDocument()
    view.rerender(dialog({ ...props, open: false }))
    view.rerender(dialog())
    expect(screen.getByTestId("qr-value")).toHaveTextContent(props.address)
    expect(
      screen.queryByRole("button", { name: "Copy pairing code" })
    ).not.toBeInTheDocument()
    expect(writeText).not.toHaveBeenCalled()
    expect(toast.success).not.toHaveBeenCalled()
  }
)

it("offers LAN pairing without relay backend and refuses a missing token", () => {
  render(
    dialog({ ...props, relayConfig: { ...relay, enabled: false }, token: "" })
  )
  expect(
    screen.queryByRole("button", { name: "Relay" })
  ).not.toBeInTheDocument()
  fireEvent.click(screen.getByRole("button", { name: "LAN + token" }))
  expect(
    screen.getByRole("button", { name: "Show pairing QR (contains token)" })
  ).toBeDisabled()
  expect(screen.queryByTestId("qr-value")).not.toBeInTheDocument()
  expect(
    screen.queryByRole("button", { name: "Copy pairing code" })
  ).not.toBeInTheDocument()
})
