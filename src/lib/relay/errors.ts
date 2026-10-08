export function relayErrorMessage(error: unknown): string {
  if (typeof error === "string") return error
  if (error && typeof error === "object" && "message" in error) {
    return String(error.message)
  }
  return ""
}

export function isRelayUnavailable(error: unknown): boolean {
  const code =
    error && typeof error === "object" && "code" in error
      ? String(error.code)
      : ""
  return (
    /^(not_implemented|command_not_found|unknown_command)$/.test(code) ||
    /not[_ ]implemented|unknown command|command.*not found/i.test(
      relayErrorMessage(error)
    )
  )
}
