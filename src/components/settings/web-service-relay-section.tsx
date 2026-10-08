"use client"

import { useEffect, useState } from "react"
import { AlertTriangle, Eye, EyeOff } from "lucide-react"
import { useTranslations } from "next-intl"
import { Badge } from "@/components/ui/badge"
import { Button } from "@/components/ui/button"
import { Input } from "@/components/ui/input"
import { Switch } from "@/components/ui/switch"
import {
  getRelayConfig,
  getRelayStatus,
  testRelayConnection,
  updateRelayConfig,
} from "@/lib/api"
import type { RelayConfig, RelayStatus, RelayTestResult } from "@/lib/types"
import {
  formatFingerprint,
  normalizeFingerprint,
} from "@/lib/relay/fingerprint"
import { isRelayUnavailable, relayErrorMessage } from "@/lib/relay/errors"

function editableConfig(config: RelayConfig) {
  return {
    enabled: config.enabled,
    url: config.url,
    key: "",
    fingerprint: formatFingerprint(config.fingerprint ?? ""),
    publicUrl: config.public_url ?? "",
  }
}

export function WebServiceRelaySection({
  onConfigChange,
}: {
  onConfigChange?: (config: RelayConfig | null) => void
}) {
  const t = useTranslations("WebServiceSettings")
  const [config, setConfig] = useState<RelayConfig | null>(null)
  const [form, setForm] = useState<ReturnType<typeof editableConfig> | null>(
    null
  )
  const [reload, setReload] = useState(0)
  const [loading, setLoading] = useState(true)
  const [unavailable, setUnavailable] = useState(false)
  const [error, setError] = useState("")
  const [saved, setSaved] = useState(false)
  const [busy, setBusy] = useState<"save" | "test" | null>(null)
  const [replacingKey, setReplacingKey] = useState(false)
  const [showKey, setShowKey] = useState(false)
  const [result, setResult] = useState<RelayTestResult | null>(null)
  const [status, setStatus] = useState<RelayStatus | null>(null)
  const [statusError, setStatusError] = useState("")

  useEffect(() => {
    let disposed = false
    setLoading(true)
    setError("")
    getRelayConfig()
      .then((next) => {
        if (disposed) return
        if (isRelayUnavailable(next)) {
          setUnavailable(true)
          onConfigChange?.(null)
          return
        }
        setConfig(next)
        setForm(editableConfig(next))
        onConfigChange?.(next)
      })
      .catch((err: unknown) => {
        if (disposed) return
        if (isRelayUnavailable(err)) setUnavailable(true)
        else setError(relayErrorMessage(err) || t("relay.failed"))
        onConfigChange?.(null)
      })
      .finally(() => {
        if (!disposed) setLoading(false)
      })
    return () => {
      disposed = true
    }
  }, [reload, onConfigChange, t])

  useEffect(() => {
    if (!config || unavailable) return
    let disposed = false
    let inFlight = false
    let timer: ReturnType<typeof setInterval> | undefined
    const isHidden = () => document.visibilityState === "hidden"
    async function poll() {
      if (disposed || inFlight || isHidden()) return
      inFlight = true
      try {
        const next = await getRelayStatus()
        if (disposed || isHidden()) return
        if (isRelayUnavailable(next)) {
          setUnavailable(true)
          onConfigChange?.(null)
          return
        }
        setStatus(next)
        setStatusError("")
      } catch (err) {
        if (disposed || isHidden()) return
        if (isRelayUnavailable(err)) {
          setUnavailable(true)
          onConfigChange?.(null)
        } else {
          setStatus(null)
          setStatusError(relayErrorMessage(err) || t("relay.failed"))
        }
      } finally {
        inFlight = false
      }
    }
    function visibilityChanged() {
      clearInterval(timer)
      if (!isHidden()) {
        void poll()
        timer = setInterval(() => void poll(), 2000)
      }
    }
    visibilityChanged()
    document.addEventListener("visibilitychange", visibilityChanged)
    return () => {
      disposed = true
      clearInterval(timer)
      document.removeEventListener("visibilitychange", visibilityChanged)
    }
  }, [config, unavailable, onConfigChange, t])

  const readOnly = config?.source === "env"
  function change(patch: Partial<NonNullable<typeof form>>) {
    setForm((previous) => previous && { ...previous, ...patch })
    setResult(null)
    setSaved(false)
    setError("")
  }

  function handleError(err: unknown) {
    if (isRelayUnavailable(err)) {
      setUnavailable(true)
      onConfigChange?.(null)
    } else setError(relayErrorMessage(err) || t("relay.failed"))
  }

  function checkedFingerprint() {
    const raw = form?.fingerprint.trim() ?? ""
    const normalized = normalizeFingerprint(raw)
    if (raw && !normalized) throw new Error(t("relay.invalidFingerprint"))
    return normalized
  }

  async function save(trustedFingerprint?: string) {
    if (!form || readOnly || busy) return
    setError("")
    setSaved(false)
    setBusy("save")
    try {
      const next = await updateRelayConfig({
        enabled: form.enabled,
        url: form.url.trim(),
        ...(form.key ? { key: form.key } : {}),
        fingerprint: trustedFingerprint ?? checkedFingerprint(),
        publicUrl: form.publicUrl.trim() || null,
      })
      if (isRelayUnavailable(next)) {
        handleError(next)
        return
      }
      setConfig(next)
      setForm(editableConfig(next))
      setReplacingKey(false)
      setShowKey(false)
      setResult(null)
      setSaved(true)
      onConfigChange?.(next)
    } catch (err) {
      handleError(err)
    } finally {
      setBusy(null)
    }
  }

  async function test() {
    if (!form || busy) return
    setError("")
    setSaved(false)
    setResult(null)
    setBusy("test")
    try {
      const next = await testRelayConnection({
        url: form.url.trim(),
        ...(form.key ? { key: form.key } : {}),
        fingerprint: checkedFingerprint(),
      })
      if (isRelayUnavailable(next)) {
        handleError(next)
        return
      }
      setResult(next)
    } catch (err) {
      handleError(err)
    } finally {
      setBusy(null)
    }
  }

  function statusLabel() {
    if (statusError) return t("relay.status.error", { error: statusError })
    if (!status) return t("relay.loading")
    switch (status.state) {
      case "connected":
        return t("relay.status.connected", {
          rtt: status.last_rtt_ms ?? "—",
          streams: status.active_streams,
        })
      case "backoff":
        return t("relay.status.backoff", {
          seconds: Math.ceil((status.next_retry_in_ms ?? 0) / 1000),
        })
      case "error":
        return t("relay.status.error", { error: status.last_error ?? "—" })
      default:
        return t(`relay.status.${status.state}`)
    }
  }

  const presented = normalizeFingerprint(result?.fingerprint ?? "")
  return (
    <section className="space-y-4 border-t pt-6" aria-labelledby="relay-title">
      <div className="space-y-1">
        <h3 id="relay-title" className="text-lg font-medium">
          {t("relay.title")}
        </h3>
        <p className="text-sm text-muted-foreground">
          {t("relay.description")}
        </p>
      </div>
      {unavailable ? (
        <p className="text-sm text-muted-foreground">
          {t("relay.unavailable")}
        </p>
      ) : loading ? (
        <p role="status" className="text-sm text-muted-foreground">
          {t("relay.loading")}
        </p>
      ) : (
        <>
          {error && (
            <p role="alert" className="text-sm text-destructive">
              {error}
            </p>
          )}
          {!form || !config ? (
            <Button
              variant="outline"
              onClick={() => setReload((value) => value + 1)}
            >
              {t("relay.retry")}
            </Button>
          ) : (
            <>
              <Badge
                role="status"
                variant={
                  status?.state === "error" || statusError
                    ? "destructive"
                    : "secondary"
                }
                className="h-auto max-w-full whitespace-normal break-words"
              >
                {statusLabel()}
              </Badge>
              {readOnly && (
                <p className="text-sm text-muted-foreground">
                  {t("relay.env")}
                </p>
              )}
              <div className="flex items-center gap-3">
                <Switch
                  id="relay-enabled"
                  checked={form.enabled}
                  disabled={readOnly || !!busy}
                  onCheckedChange={(enabled) => change({ enabled })}
                />
                <label htmlFor="relay-enabled" className="text-sm font-medium">
                  {t("relay.enable")}
                </label>
              </div>
              <div className="space-y-1.5">
                <label htmlFor="relay-url" className="text-sm font-medium">
                  {t("relay.url")}
                </label>
                <Input
                  id="relay-url"
                  dir="ltr"
                  value={form.url}
                  readOnly={readOnly}
                  disabled={!!busy}
                  onChange={(event) => change({ url: event.target.value })}
                  placeholder="https://203.0.113.10:18443"
                  autoComplete="off"
                  spellCheck={false}
                />
              </div>
              <div className="space-y-1.5">
                <div className="flex items-center gap-3">
                  <label htmlFor="relay-key" className="text-sm font-medium">
                    {t("relay.key")}
                  </label>
                  {config.key_set && (
                    <span className="text-xs text-muted-foreground">
                      {t("relay.keySet")}
                    </span>
                  )}
                  {config.key_set && !replacingKey && !readOnly && (
                    <Button
                      variant="ghost"
                      size="sm"
                      disabled={!!busy}
                      onClick={() => setReplacingKey(true)}
                    >
                      {t("relay.replace")}
                    </Button>
                  )}
                </div>
                <div className="flex items-center gap-2">
                  <Input
                    id="relay-key"
                    dir="ltr"
                    type={showKey ? "text" : "password"}
                    value={form.key}
                    readOnly={readOnly || (config.key_set && !replacingKey)}
                    disabled={!!busy}
                    onChange={(event) => change({ key: event.target.value })}
                    autoComplete="new-password"
                    spellCheck={false}
                  />
                  {!readOnly && (!config.key_set || replacingKey) && (
                    <Button
                      variant="ghost"
                      size="icon"
                      disabled={!!busy}
                      aria-label={showKey ? t("hide") : t("show")}
                      onClick={() => setShowKey((value) => !value)}
                    >
                      {showKey ? <EyeOff /> : <Eye />}
                    </Button>
                  )}
                </div>
              </div>
              <div className="space-y-1.5">
                <label
                  htmlFor="relay-fingerprint"
                  className="text-sm font-medium"
                >
                  {t("relay.fingerprint")}
                </label>
                <Input
                  id="relay-fingerprint"
                  dir="ltr"
                  className="font-mono text-xs"
                  value={form.fingerprint}
                  readOnly={readOnly}
                  disabled={!!busy}
                  onChange={(event) =>
                    change({ fingerprint: event.target.value })
                  }
                  onBlur={() => {
                    const normalized = normalizeFingerprint(form.fingerprint)
                    if (normalized)
                      setForm({
                        ...form,
                        fingerprint: formatFingerprint(normalized),
                      })
                  }}
                  autoComplete="off"
                  spellCheck={false}
                />
                <p className="text-xs text-muted-foreground">
                  {t("relay.fingerprintHint")}
                </p>
              </div>
              <details className="space-y-3">
                <summary className="cursor-pointer text-sm font-medium">
                  {t("relay.advanced")}
                </summary>
                <div className="space-y-1.5">
                  <label
                    htmlFor="relay-public-url"
                    className="text-sm font-medium"
                  >
                    {t("relay.publicUrl")}
                  </label>
                  <Input
                    id="relay-public-url"
                    dir="ltr"
                    value={form.publicUrl}
                    readOnly={readOnly}
                    disabled={!!busy}
                    onChange={(event) =>
                      change({ publicUrl: event.target.value })
                    }
                    autoComplete="off"
                    spellCheck={false}
                  />
                  <p className="text-xs text-muted-foreground">
                    {t("relay.publicUrlHint")}
                  </p>
                </div>
              </details>
              {result &&
                (result.ok ? (
                  <p
                    role="status"
                    className="text-sm text-green-600 dark:text-green-400"
                  >
                    {t("relay.testSuccess", { rtt: result.rtt_ms ?? "—" })}
                  </p>
                ) : (
                  <div
                    role="alert"
                    className={`space-y-3 rounded-lg border p-3 text-sm ${result.error_code === "fingerprint_mismatch" ? "border-destructive bg-destructive/10 text-destructive" : "border-amber-500/40 bg-amber-500/10"}`}
                  >
                    <div className="flex items-start gap-2">
                      <AlertTriangle className="mt-0.5 size-4 shrink-0" />
                      <p>
                        {result.error_code === "fingerprint_mismatch"
                          ? t("relay.mismatch")
                          : result.error_code === "tls_untrusted"
                            ? t("relay.untrusted")
                            : result.error || t("relay.failed")}
                      </p>
                    </div>
                    {presented && (
                      <code dir="ltr" className="block break-all select-all">
                        {formatFingerprint(presented)}
                      </code>
                    )}
                    {result.error_code === "tls_untrusted" &&
                      presented &&
                      !readOnly && (
                        <Button
                          variant="outline"
                          disabled={!!busy}
                          onClick={() => void save(presented)}
                        >
                          {t("relay.trust")}
                        </Button>
                      )}
                  </div>
                ))}
              <div className="flex flex-wrap items-center gap-2">
                <Button
                  disabled={
                    readOnly || !!busy || result?.error_code === "tls_untrusted"
                  }
                  onClick={() => void save()}
                >
                  {t("relay.save")}
                </Button>
                <Button
                  variant="outline"
                  disabled={!!busy || !form.url.trim()}
                  onClick={() => void test()}
                >
                  {busy === "test" ? t("relay.testing") : t("relay.test")}
                </Button>
                {busy === "save" && (
                  <span role="status" className="text-sm text-muted-foreground">
                    {t("processing")}
                  </span>
                )}
                {saved && (
                  <span role="status" className="text-sm text-muted-foreground">
                    {t("relay.saved")}
                  </span>
                )}
              </div>
            </>
          )}
        </>
      )}
    </section>
  )
}
