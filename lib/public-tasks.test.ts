import { describe, expect, it } from "vitest";

import { renderRss, renderTaskPage } from "@/lib/public-tasks";

describe("public task rendering", () => {
  it("renders task links into rss xml", () => {
    const rss = renderRss("https://example.com", [
      {
        taskId: "task-123",
        title: "Example",
        summary: "Summary",
        completedAt: "2026-03-22T12:00:00Z",
        updatedAt: "2026-03-22T12:00:00Z",
      },
    ]);

    expect(rss).toContain("<link>https://example.com/tasks/task-123</link>");
    expect(rss).toContain("<title>Example</title>");
  });

  it("escapes html in task pages", () => {
    const page = renderTaskPage({
      taskId: "task-123",
      title: "<Example>",
      summary: "A&B",
      completedAt: null,
      updatedAt: "2026-03-22T12:00:00Z",
    });

    expect(page).toContain("&lt;Example&gt;");
    expect(page).toContain("A&amp;B");
  });
});
