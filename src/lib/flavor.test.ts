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

  it("disables official update checks without changing identity", async () => {
    vi.stubEnv("NEXT_PUBLIC_CODEG_FLAVOR", undefined)
    vi.stubEnv("NEXT_PUBLIC_CODEG_UPDATER", "off")
    const flavor = await import("./flavor")
    expect(flavor.UPDATER_ENABLED).toBe(false)
    expect(flavor.PRODUCT_NAME).toBe("codeg")
    expect(flavor.DEFAULT_WEB_PORT).toBe(3080)
    const { checkAppUpdateInfo } = await import("./updater")
    await expect(checkAppUpdateInfo()).resolves.toEqual({
      currentVersion: "0.34.0",
      update: null,
    })
    expect(manifestCheck).not.toHaveBeenCalled()
  })

  it("blocks selfhost install and rollback without contacting the backend", async () => {
    vi.stubEnv("NEXT_PUBLIC_CODEG_UPDATER", "off")
    const { startAppUpdate, rollbackServer } = await import("./updater")
    await expect(startAppUpdate()).rejects.toThrow("Updates are disabled")
    await expect(rollbackServer()).rejects.toThrow("Updates are disabled")
  })

  it.each([undefined, "", "on", "OFF"])(
    "keeps official updates enabled for %s",
    async (value) => {
      vi.stubEnv("NEXT_PUBLIC_CODEG_FLAVOR", undefined)
      vi.stubEnv("NEXT_PUBLIC_CODEG_UPDATER", value)
      expect((await import("./flavor")).UPDATER_ENABLED).toBe(true)
    }
  )

  it("stamps the display version without changing the raw app version", async () => {
    vi.stubEnv("NEXT_PUBLIC_CODEG_BUILD_COMMIT", "1234abcd")
    const { formatBuildVersion } = await import("./flavor")
    const { getCurrentAppVersion } = await import("./updater")
    expect(formatBuildVersion("0.34.0")).toBe("0.34.0 (fork 1234abcd)")
    await expect(getCurrentAppVersion()).resolves.toBe("0.34.0")
  })

  it("keeps unstamped version display unchanged", async () => {
    vi.stubEnv("NEXT_PUBLIC_CODEG_BUILD_COMMIT", undefined)
    const { formatBuildVersion } = await import("./flavor")
    expect(formatBuildVersion("0.34.0")).toBe("0.34.0")
  })
})
