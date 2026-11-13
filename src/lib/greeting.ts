// The time-of-day greeting and the daily 「」 quote on the Home page.

export function greetingKey(hour: number): string {
  if (hour >= 5 && hour <= 10) return "home.greeting.morning";
  if (hour >= 11 && hour <= 12) return "home.greeting.noon";
  if (hour >= 13 && hour <= 17) return "home.greeting.afternoon";
  if (hour >= 18 && hour <= 22) return "home.greeting.evening";
  return "home.greeting.night";
}

export const QUOTE_COUNT = 5;

/** The same day always gives the same quote; the quote changes every day. */
export function quoteKey(date: Date): string {
  const start = Date.UTC(date.getFullYear(), 0, 1);
  const today = Date.UTC(date.getFullYear(), date.getMonth(), date.getDate());
  const dayOfYear = Math.round((today - start) / 86_400_000);
  return `home.quote.${(dayOfYear % QUOTE_COUNT) + 1}`;
}
