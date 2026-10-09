// Static export embeds this choice; build-fork.sh sets it with CODEG_FLAVOR.
export const IS_FORK = process.env.NEXT_PUBLIC_CODEG_FLAVOR === "fork"
export const PRODUCT_NAME = IS_FORK ? "Codeg-Fork" : "codeg"
export const NOTIFICATION_PRODUCT_NAME = IS_FORK ? "Codeg-Fork" : "Codeg"
export const DEFAULT_WEB_PORT = IS_FORK ? 3090 : 3080
export const UPDATER_ENABLED =
  !IS_FORK && process.env.NEXT_PUBLIC_CODEG_UPDATER !== "off"

export function formatBuildVersion(version: string): string {
  const commit = process.env.NEXT_PUBLIC_CODEG_BUILD_COMMIT
  return commit ? `${version} (fork ${commit})` : version
}
