import { getNotionConfig } from "@/lib/config";
import type { PublicTaskSummary } from "@/lib/public-tasks";

const NOTION_VERSION = "2022-06-28";
const NOTION_API_BASE = "https://api.notion.com/v1";

interface NotionQueryResponse {
  results?: NotionPage[];
}

interface NotionPage {
  properties?: Record<string, unknown>;
}

export async function queryPublishedTasks(
  limit = 50,
): Promise<PublicTaskSummary[]> {
  const { token, databaseId } = getNotionConfig();
  const response = await fetch(`${NOTION_API_BASE}/databases/${databaseId}/query`, {
    method: "POST",
    headers: notionHeaders(token),
    body: JSON.stringify({
      page_size: limit,
      filter: {
        property: "Publish",
        checkbox: { equals: true },
      },
      sorts: [
        {
          property: "Updated At",
          direction: "descending",
        },
      ],
    }),
    cache: "no-store",
  });

  if (!response.ok) {
    throw new Error(`notion query returned error: ${response.status}`);
  }

  const body = (await response.json()) as NotionQueryResponse;
  return extractPublishedTasks(body);
}

export async function fetchPublicTask(
  taskId: string,
): Promise<PublicTaskSummary | null> {
  const tasks = await queryPublishedTasks(100);
  return tasks.find((task) => task.taskId === taskId) ?? null;
}

export function extractPublishedTasks(
  body: NotionQueryResponse,
): PublicTaskSummary[] {
  const results = body.results ?? [];
  const tasks: PublicTaskSummary[] = [];

  for (const page of results) {
    const properties = page.properties ?? {};
    const taskId = extractPlainText(properties, "Task ID");

    if (!taskId) {
      continue;
    }

    tasks.push({
      taskId,
      title: extractPlainText(properties, "Title"),
      summary: extractPlainText(properties, "Public Summary"),
      completedAt: extractDate(properties, "Completed At"),
      updatedAt: extractDate(properties, "Updated At") ?? "",
    });
  }

  return tasks;
}

function notionHeaders(token: string): HeadersInit {
  return {
    Authorization: `Bearer ${token}`,
    "Content-Type": "application/json",
    "Notion-Version": NOTION_VERSION,
  };
}

type NotionPropertyMap = Record<string, unknown>;

type NotionTextItem = {
  plain_text?: string;
  text?: {
    content?: string;
  };
};

function extractPlainText(properties: NotionPropertyMap, key: string): string {
  const property = properties[key] as
    | {
        rich_text?: NotionTextItem[];
        title?: NotionTextItem[];
      }
    | undefined;

  const items = property?.rich_text ?? property?.title ?? [];
  return items
    .map((item) => item.plain_text ?? item.text?.content ?? "")
    .join("");
}

function extractDate(
  properties: NotionPropertyMap,
  key: string,
): string | null {
  const property = properties[key] as
    | {
        date?: {
          start?: string;
        } | null;
      }
    | undefined;

  return property?.date?.start ?? null;
}
