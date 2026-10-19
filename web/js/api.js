export async function invokeOrFetch(command, args, endpoint, method = "GET") {
  const invoke = window.__TAURI__?.core?.invoke;
  if (invoke) return invoke(command, args);

  const response = await fetch(endpoint, {
    method,
    cache: "no-store",
    ...(args ? {
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify(args),
    } : {}),
  });
  const responseText = await response.text();
  if (!response.ok) {
    throw new Error(responseText || `HTTP ${response.status}`);
  }
  if (!responseText.trim()) return null;

  try {
    return JSON.parse(responseText);
  } catch {
    throw new Error("Backend вернул некорректный JSON");
  }
}
