import { describe, expect, it, vi, afterEach } from "vitest"

const { manifestCheck } = vi.hoisted(() => ({ manifestCheck: vi.fn() }))
vi.mock("./transport", () => ({
  isDesktop: () => true,
  isRemoteDesktopMode: () => false,
}))
vi.mock("@tauri-apps/api/app", () => ({ getVersion: async () => "0.34.0" }))
vi.mock("@tauri-apps/plugin-updater", () => ({ check: manifestCheck }))

afterEach(() => {
  vi.unstubAllEnvs()
  vi.resetModules()
})

describe("build flavor", () => {
  it.each([undefined, "", "official", "FORK"])(
    "preserves official labels and defaults for %s",
    async (value) => {
      vi.stubEnv("NEXT_PUBLIC_CODEG_FLAVOR", value)
      const flavor = await import("./flavor")
      expect(flavor.PRODUCT_NAME).toBe("codeg")
      expect(flavor.NOTIFICATION_PRODUCT_NAME).toBe("Codeg")
      expect(flavor.DEFAULT_WEB_PORT).toBe(3080)
      expect(flavor.UPDATER_ENABLED).toBe(true)
    }
  )

  it("isolates the fork UI and disables update checks", async () => {
    vi.stubEnv("NEXT_PUBLIC_CODEG_FLAVOR", "fork")
    const flavor = await import("./flavor")
    expect(flavor.PRODUCT_NAME).toBe("Codeg-Fork")
    expect(flavor.NOTIFICATION_PRODUCT_NAME).toBe("Codeg-Fork")
    expect(flavor.DEFAULT_WEB_PORT).toBe(3090)
    expect(flavor.UPDATER_ENABLED).toBe(false)
  })

  it("returns the local version without invoking the updater in Fork", async () => {
    vi.stubEnv("NEXT_PUBLIC_CODEG_FLAVOR", "fork")
    const { checkAppUpdateInfo } = await import("./updater")
    await expect(checkAppUpdateInfo()).resolves.toEqual({
      currentVersion: "0.34.0",
      update: null,
    })
    expect(manifestCheck).not.toHaveBeenCalled()
  })
})
