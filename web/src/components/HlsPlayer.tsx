// Thin wrapper around HLS.js with native-HLS fallback for Safari.
// Used by the live camera grid (live HLS) and the recording-playback
// modal (VOD HLS) — same component, different src URLs.

import { useEffect, useRef, useState } from "react"
import Hls from "hls.js"

interface HlsPlayerProps {
  src: string
  className?: string
  autoPlay?: boolean
  muted?: boolean
  controls?: boolean
}

export default function HlsPlayer({
  src,
  className,
  autoPlay = true,
  muted = true,
  controls = false,
}: HlsPlayerProps) {
  const videoRef = useRef<HTMLVideoElement>(null)
  // `nonce` increments on retry-click and is appended as a cache buster
  // so HLS.js (and any intermediate proxy) refetches the manifest from
  // scratch instead of replaying the failed cached response.
  const [nonce, setNonce] = useState(0)
  const [error, setError] = useState<string | null>(null)

  useEffect(() => {
    const video = videoRef.current
    if (!video) return

    setError(null)

    // Append the retry nonce so a "Retry" click after a failure forces
    // a fresh manifest load.  Order: existing query params first.
    const url = nonce > 0
      ? `${src}${src.includes("?") ? "&" : "?"}_t=${nonce}`
      : src

    // Native HLS (Safari + iOS) — feed the URL directly.
    if (video.canPlayType("application/vnd.apple.mpegurl")) {
      video.src = url
      if (autoPlay) {
        void video.play().catch(() => {
          // Autoplay can be blocked; the controls (or user interaction)
          // will resume.  Don't surface an error for this.
        })
      }
      return () => {
        video.removeAttribute("src")
        video.load()
      }
    }

    // hls.js path (Chrome / Firefox / Edge).
    if (Hls.isSupported()) {
      const hls = new Hls({
        // Live tuning.  These defaults are tolerant of the first ~3 s
        // after a camera starts when FFmpeg hasn't written the first
        // segment yet — we want to wait it out, not surface a black
        // tile.  `manifestLoadingMaxRetry: 6` ≈ 6 retries with hls.js's
        // default exponential backoff (~64 s ceiling) before giving up.
        liveSyncDurationCount: 3,
        manifestLoadingTimeOut: 10_000,
        manifestLoadingMaxRetry: 6,
        manifestLoadingRetryDelay: 1_000,
        levelLoadingMaxRetry: 6,
        levelLoadingRetryDelay: 1_000,
      })
      hls.loadSource(url)
      hls.attachMedia(video)
      hls.on(Hls.Events.MEDIA_ATTACHED, () => {
        if (autoPlay) {
          void video.play().catch(() => undefined)
        }
      })
      hls.on(Hls.Events.ERROR, (_evt, data) => {
        // Only surface fatal errors — non-fatal recovers automatically.
        if (!data.fatal) return
        // Map the HLS.js error type to a short human-readable string.
        const detail = data.details ?? data.type ?? "playback error"
        setError(`${detail}`)
        hls.destroy()
      })
      return () => {
        hls.destroy()
      }
    }

    // Final fallback: just set the src and hope the browser figures
    // it out.  Nothing else to wire up.
    video.src = url
    return () => {
      video.removeAttribute("src")
      video.load()
    }
  }, [src, autoPlay, nonce])

  if (error) {
    return (
      <div className={className} style={{
        display: "flex",
        flexDirection: "column",
        alignItems: "center",
        justifyContent: "center",
        gap: "0.75rem",
        background: "var(--bg-secondary)",
        color: "var(--text-secondary)",
        fontSize: "0.85rem",
        padding: "1rem",
        textAlign: "center",
      }}>
        <div style={{ color: "var(--accent-red)", fontWeight: 600 }}>
          Stream unavailable
        </div>
        <div style={{ color: "var(--text-muted)", fontSize: "0.75rem" }}>
          {error}
        </div>
        <button
          type="button"
          className="btn"
          onClick={() => setNonce(n => n + 1)}
        >
          Retry
        </button>
      </div>
    )
  }

  return (
    <video
      ref={videoRef}
      className={className}
      muted={muted}
      controls={controls}
      playsInline
    />
  )
}
