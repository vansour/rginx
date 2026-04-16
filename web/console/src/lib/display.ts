export type StreamState = "idle" | "connecting" | "live" | "reconnecting" | "error";

const unixMsFormatter = new Intl.DateTimeFormat("zh-CN", {
  year: "numeric",
  month: "2-digit",
  day: "2-digit",
  hour: "2-digit",
  minute: "2-digit",
  second: "2-digit",
  hour12: false,
});

export function formatUnixMs(value: number | null | undefined): string {
  if (value === null || value === undefined) {
    return "-";
  }

  return unixMsFormatter.format(new Date(value));
}

export function formatList(values: Array<string | number> | null | undefined): string {
  if (!values || values.length === 0) {
    return "-";
  }

  return values.join(", ");
}

export function formatNullable(value: string | number | boolean | null | undefined): string {
  if (value === null || value === undefined || value === "") {
    return "-";
  }

  return String(value);
}

export function formatBoolean(
  value: boolean | null | undefined,
  trueLabel = "enabled",
  falseLabel = "disabled",
): string {
  if (value === null || value === undefined) {
    return "-";
  }

  return value ? trueLabel : falseLabel;
}

export function streamStateLabel(state: StreamState): string {
  switch (state) {
    case "connecting":
      return "connecting";
    case "live":
      return "live";
    case "reconnecting":
      return "reconnecting";
    case "error":
      return "error";
    default:
      return "idle";
  }
}
