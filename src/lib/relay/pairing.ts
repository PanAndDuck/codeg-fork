export function buildPairingPayload({
  mode,
  url,
  token,
  fp,
  name,
}: {
  mode: "relay" | "direct"
  url: string
  token: string
  fp?: string
  name?: string
}): string {
  return JSON.stringify({ v: 1, mode, url, token, fp, name })
}
