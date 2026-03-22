import { describe, expect, it } from "vitest";

import { extractPublishedTasks } from "@/lib/notion";

describe("extractPublishedTasks", () => {
  it("maps notion properties into public task summaries", () => {
    const tasks = extractPublishedTasks({
      results: [
        {
          properties: {
            "Task ID": {
              rich_text: [{ plain_text: "task-123" }],
            },
            Title: {
              title: [{ plain_text: "Example" }],
            },
            "Public Summary": {
              rich_text: [{ plain_text: "Summary" }],
            },
            "Completed At": {
              date: { start: "2026-03-22T12:00:00Z" },
            },
            "Updated At": {
              date: { start: "2026-03-22T13:00:00Z" },
            },
          },
        },
      ],
    });

    expect(tasks).toEqual([
      {
        taskId: "task-123",
        title: "Example",
        summary: "Summary",
        completedAt: "2026-03-22T12:00:00Z",
        updatedAt: "2026-03-22T13:00:00Z",
      },
    ]);
  });
});
