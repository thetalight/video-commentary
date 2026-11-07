import { useEffect, useRef, useState } from "react";
import { episodeMigrationStatus, migrateEpisode } from "../lib/api";
import type { Episode } from "../lib/types";

/** Local feedback stays beside the button: no WebView confirm() dependency. */
export function LegacyMigrationPanel({ taskId, revision, disabledReason, onMigrated, onRegenerate, onBusyChange }: {
  taskId: string;
  revision: string;
  disabledReason: string;
  onMigrated: (episode: Episode) => void;
  onRegenerate: () => void;
  onBusyChange: (busy: boolean) => void;
}) {
  const [status, setStatus] = useState<{ can_migrate: boolean; reason: string } | null>(null);
  const [checking, setChecking] = useState(false);
  const [confirmed, setConfirmed] = useState(false);
  const [busy, setBusy] = useState(false);
  const [slow, setSlow] = useState(false);
  const [error, setError] = useState("");
  const [retry, setRetry] = useState(0);
  const mounted = useRef(true);
  const inFlight = useRef(false);
  useEffect(() => {
    mounted.current = true;
    return () => { mounted.current = false; };
  }, []);
  useEffect(() => {
    let active = true;
    setStatus(null); setConfirmed(false); setError("");
    if (disabledReason) { setChecking(false); return; }
    setChecking(true);
    let timer: ReturnType<typeof setTimeout>;
    Promise.race([
      episodeMigrationStatus(taskId, revision),
      new Promise<never>((_, reject) => { timer = setTimeout(() => reject(new Error("迁移条件检查超过15秒。尚未执行迁移，请重新检查或确认桌面后端仍在运行。")), 15000); }),
    ])
      .then(result => { if (active) setStatus(result); })
      .catch(e => { if (active) setError(`无法检查迁移条件：${String(e)}`); })
      .finally(() => { clearTimeout(timer); if (active) setChecking(false); });
    return () => { active = false; clearTimeout(timer); };
  }, [taskId, revision, retry, disabledReason]);
  useEffect(() => {
    if (!busy) { setSlow(false); return; }
    const timer = window.setTimeout(() => setSlow(true), 8000);
    return () => window.clearTimeout(timer);
  }, [busy]);
  const blocked = disabledReason || (checking ? "正在检查原片、字幕和选片范围…" : "");
  async function migrate() {
    if (inFlight.current || blocked || !status?.can_migrate || !confirmed) return;
    inFlight.current = true; setBusy(true); onBusyChange(true); setError("");
    try {
      const next = await migrateEpisode(taskId, revision);
      if (next.schema_version !== 2) throw new Error("后端没有返回v2稿件，未完成迁移");
      if (mounted.current) onMigrated(next);
    } catch (e) {
      if (mounted.current) { setError(`迁移未完成：${String(e)}`); setConfirmed(false); }
    } finally {
      inFlight.current = false;
      if (mounted.current) { setBusy(false); onBusyChange(false); }
    }
  }
  return <section className="warning" aria-label="旧稿迁移操作">
    <h4>旧稿需要升级或重新生成</h4>
    <p>迁移只复制旧稿证据和画面选择，不调用AI、不重新下载或转写。原稿先归档；旧评审和成片引用失效，必保区间需重新核对。</p>
    {blocked && <p role="status">{blocked}</p>}
    {!blocked && status && <p role="status">{status.reason}</p>}
    {error && <p role="alert" className="error-text">{error}</p>}
    {busy ? <div aria-live="polite" aria-busy="true">
      <p>迁移请求已发送，正在检查素材、归档并保存v2，请勿重复操作。</p>
      {slow && <p>等待时间较长，操作尚未确认完成。请不要重复点击；如后端报错，原因会显示在这里。</p>}
    </div> : !blocked && status?.can_migrate ? <>
      <label><input type="checkbox" checked={confirmed} onChange={e => setConfirmed(e.target.checked)} />确认归档原稿并清除旧评审；迁移后重新核对和审稿。</label>
      <button type="button" className="primary" disabled={!confirmed} onClick={() => void migrate()}>归档并迁移到v2</button>
    </> : null}
    {!busy && !blocked && status && !status.can_migrate && <button type="button" className="primary" onClick={onRegenerate}>材料已变化，改为重新生成整集稿</button>}
    {!busy && !disabledReason && <button type="button" disabled={checking} onClick={() => setRetry(n => n + 1)}>重新检查迁移条件</button>}
  </section>;
}
