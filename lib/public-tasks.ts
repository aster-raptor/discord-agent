export interface PublicTaskSummary {
  taskId: string;
  title: string;
  summary: string;
  completedAt?: string | null;
  updatedAt: string;
}

export function xmlEscape(value: string): string {
  return value
    .replaceAll("&", "&amp;")
    .replaceAll("<", "&lt;")
    .replaceAll(">", "&gt;")
    .replaceAll('"', "&quot;")
    .replaceAll("'", "&apos;");
}

export function htmlEscape(value: string): string {
  return xmlEscape(value);
}

export function renderRss(
  baseUrl: string,
  tasks: PublicTaskSummary[],
): string {
  const items = tasks
    .map((task) => {
      const link = `${baseUrl}/tasks/${task.taskId}`;
      const publishedAt = task.completedAt ?? task.updatedAt;

      return `<item><title>${xmlEscape(task.title)}</title><link>${xmlEscape(link)}</link><guid>${xmlEscape(task.taskId)}</guid><description>${xmlEscape(task.summary)}</description><pubDate>${xmlEscape(publishedAt)}</pubDate></item>`;
    })
    .join("");

  return `<?xml version="1.0" encoding="UTF-8"?><rss version="2.0"><channel><title>discord-agent reports</title><link>${xmlEscape(baseUrl)}</link><description>Public summaries generated from private Notion task reports.</description>${items}</channel></rss>`;
}

export function renderTaskPage(task: PublicTaskSummary): string {
  const updatedAt = task.completedAt ?? task.updatedAt;

  return `<!DOCTYPE html><html lang="ja"><head><meta charset="utf-8"><title>${htmlEscape(task.title)}</title></head><body><main><h1>${htmlEscape(task.title)}</h1><p><strong>Task ID:</strong> ${htmlEscape(task.taskId)}</p><p><strong>Updated:</strong> ${htmlEscape(updatedAt)}</p><article><pre style="white-space: pre-wrap;">${htmlEscape(task.summary)}</pre></article></main></body></html>`;
}
