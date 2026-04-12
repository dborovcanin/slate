function pad2(value: number): string {
  return value < 10 ? `0${value}` : `${value}`;
}

const monthNamesShort = [
  "Jan",
  "Feb",
  "Mar",
  "Apr",
  "May",
  "Jun",
  "Jul",
  "Aug",
  "Sep",
  "Oct",
  "Nov",
  "Dec",
];

const monthNamesLong = [
  "January",
  "February",
  "March",
  "April",
  "May",
  "June",
  "July",
  "August",
  "September",
  "October",
  "November",
  "December",
];

export function formatDateByPattern(date: Date, pattern: string): string {
  const yyyy = `${date.getFullYear()}`;
  const yy = yyyy.slice(-2);
  const mm = pad2(date.getMonth() + 1);
  const m = `${date.getMonth() + 1}`;
  const dd = pad2(date.getDate());
  const d = `${date.getDate()}`;
  const MMM = monthNamesShort[date.getMonth()];
  const MMMM = monthNamesLong[date.getMonth()];

  const out = pattern || "%Y-%m-%d";

  if (out.includes("%")) {
    const replacements: [string, string][] = [
      ["%Y", yyyy],
      ["%y", yy],
      ["%m", mm],
      ["%d", dd],
      ["%b", MMM],
      ["%B", MMMM],
    ];
    let result = out;
    for (const [token, value] of replacements) {
      result = result.split(token).join(value);
    }
    return result;
  }

  const replacements: [string, string][] = [
    ["YYYY", yyyy],
    ["MMMM", MMMM],
    ["MMM", MMM],
    ["MM", mm],
    ["DD", dd],
    ["YY", yy],
    ["M", m],
    ["D", d],
  ];
  let result = out;
  for (const [token, value] of replacements) {
    result = result.split(token).join(value);
  }
  return result;
}

function todayIsoDate(): string {
  const now = new Date();
  return `${now.getFullYear()}-${pad2(now.getMonth() + 1)}-${pad2(now.getDate())}`;
}

export function openDatePicker(format: string): Promise<string | null> {
  return new Promise((resolve) => {
    const overlay = document.createElement("div");
    overlay.className = "date-picker-overlay";

    const panel = document.createElement("div");
    panel.className = "date-picker-panel";

    const title = document.createElement("div");
    title.className = "date-picker-title";
    title.textContent = "Choose date";

    const input = document.createElement("input");
    input.className = "date-picker-input";
    input.type = "date";
    input.value = todayIsoDate();

    const actions = document.createElement("div");
    actions.className = "date-picker-actions";

    const cancelBtn = document.createElement("button");
    cancelBtn.className = "date-picker-btn date-picker-btn-secondary";
    cancelBtn.textContent = "Cancel";

    const insertBtn = document.createElement("button");
    insertBtn.className = "date-picker-btn date-picker-btn-primary";
    insertBtn.textContent = "Insert";

    actions.appendChild(cancelBtn);
    actions.appendChild(insertBtn);
    panel.appendChild(title);
    panel.appendChild(input);
    panel.appendChild(actions);
    overlay.appendChild(panel);
    document.body.appendChild(overlay);

    const close = (result: string | null) => {
      overlay.remove();
      resolve(result);
    };

    overlay.addEventListener("mousedown", (e) => {
      if (e.target === overlay) close(null);
    });

    cancelBtn.addEventListener("click", () => close(null));

    insertBtn.addEventListener("click", () => {
      if (!input.value) {
        close(null);
        return;
      }
      const parsed = new Date(`${input.value}T00:00:00`);
      close(formatDateByPattern(parsed, format));
    });

    input.addEventListener("keydown", (e) => {
      if (e.key === "Escape") {
        e.preventDefault();
        close(null);
      } else if (e.key === "Enter") {
        e.preventDefault();
        insertBtn.click();
      }
    });

    window.setTimeout(() => input.focus(), 0);
  });
}
