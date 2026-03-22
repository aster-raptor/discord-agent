import { getPublicBaseUrl } from "@/lib/config";
import { queryPublishedTasks } from "@/lib/notion";
import { renderRss } from "@/lib/public-tasks";

export const runtime = "nodejs";
export const dynamic = "force-dynamic";

export async function GET() {
  try {
    const tasks = await queryPublishedTasks(50);
    const rss = renderRss(getPublicBaseUrl(), tasks);

    return new Response(rss, {
      headers: {
        "Content-Type": "application/rss+xml; charset=utf-8",
      },
    });
  } catch (error) {
    return new Response(`failed to query notion: ${formatError(error)}`, {
      status: 502,
      headers: {
        "Content-Type": "text/plain; charset=utf-8",
      },
    });
  }
}

function formatError(error: unknown): string {
  return error instanceof Error ? error.message : "unknown error";
}