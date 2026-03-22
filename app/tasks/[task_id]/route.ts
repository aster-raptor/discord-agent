import { fetchPublicTask } from "@/lib/notion";
import { renderTaskPage } from "@/lib/public-tasks";

export const runtime = "nodejs";
export const dynamic = "force-dynamic";

export async function GET(
  _request: Request,
  { params }: { params: Promise<{ task_id: string }> },
) {
  try {
    const { task_id } = await params;
    const task = await fetchPublicTask(task_id);

    if (!task) {
      return new Response("task not found", {
        status: 404,
        headers: {
          "Content-Type": "text/plain; charset=utf-8",
        },
      });
    }

    return new Response(renderTaskPage(task), {
      headers: {
        "Content-Type": "text/html; charset=utf-8",
      },
    });
  } catch (error) {
    return new Response(`failed to fetch task: ${formatError(error)}`, {
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