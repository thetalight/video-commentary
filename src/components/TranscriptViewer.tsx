import { useEffect, useState } from "react";
import { readJobTranscript } from "../lib/api";

interface TranscriptViewerProps {
  taskId: string;
  source?: string;
}

export function TranscriptViewer({ taskId, source }: TranscriptViewerProps) {
  const [open, setOpen] = useState(false);
  const [transcript, setTranscript] = useState<string | null>(null);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    setOpen(false);
    setTranscript(null);
    setError(null);
  }, [taskId]);

  async function handleToggle() {
    const nextOpen = !open;
    setOpen(nextOpen);
    if (!nextOpen || loading) return;

    setLoading(true);
    setError(null);
    try {
      setTranscript(await readJobTranscript(taskId));
    } catch (reason) {
      setError(String(reason));
    } finally {
      setLoading(false);
    }
  }

  return (
    <section className="transcript-panel">
      <button
        type="button"
        className="transcript-toggle"
        onClick={handleToggle}
        aria-expanded={open}
      >
        <span>原字幕{source ? ` · ${source}` : ""}</span>
        <span>{open ? "收起" : "查看"}</span>
      </button>
      {open && (
        <div className="transcript-content">
          {loading && <p>正在读取字幕...</p>}
          {error && <p className="online-task-error">{error}</p>}
          {transcript !== null && <pre>{transcript}</pre>}
        </div>
      )}
    </section>
  );
}
