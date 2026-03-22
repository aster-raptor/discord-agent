export function getNotionConfig() {
  const token = process.env.NOTION_TOKEN?.trim();
  const databaseId = process.env.NOTION_TASK_DATABASE_ID?.trim();

  if (!token || !databaseId) {
    throw new Error(
      "NOTION_TOKEN and NOTION_TASK_DATABASE_ID are required for the public app",
    );
  }

  return { token, databaseId };
}

export function getPublicBaseUrl(): string {
  return process.env.PUBLIC_BASE_URL?.trim() || "http://localhost:3000";
}
