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

const dayHeaders = ["Mo", "Tu", "We", "Th", "Fr", "Sa", "Su"];

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

function daysInMonth(year: number, month: number): number {
  return new Date(year, month + 1, 0).getDate();
}

/** 0=Mon, 1=Tue, ..., 6=Sun */
function startDayOfWeek(year: number, month: number): number {
  const d = new Date(year, month, 1).getDay(); // 0=Sun
  return (d + 6) % 7;
}

export function openDatePicker(format: string): Promise<string | null> {
  const now = new Date();
  let year = now.getFullYear();
  let month = now.getMonth(); // 0-based
  let day = now.getDate();

  return new Promise((resolve) => {
    const overlay = document.createElement("div");
    overlay.className = "date-picker-overlay";

    const panel = document.createElement("div");
    panel.className = "date-picker-panel";

    // Header row: < Month Year >
    const header = document.createElement("div");
    header.className = "date-picker-header";

    const prevBtn = document.createElement("button");
    prevBtn.className = "date-picker-nav";
    prevBtn.textContent = "‹";
    prevBtn.setAttribute("aria-label", "Previous month");

    const monthLabel = document.createElement("span");
    monthLabel.className = "date-picker-month-label";

    const nextBtn = document.createElement("button");
    nextBtn.className = "date-picker-nav";
    nextBtn.textContent = "›";
    nextBtn.setAttribute("aria-label", "Next month");

    header.appendChild(prevBtn);
    header.appendChild(monthLabel);
    header.appendChild(nextBtn);

    // Day-of-week header
    const dowRow = document.createElement("div");
    dowRow.className = "date-picker-dow";
    for (const dh of dayHeaders) {
      const cell = document.createElement("span");
      cell.className = "date-picker-dow-cell";
      cell.textContent = dh;
      dowRow.appendChild(cell);
    }

    // Grid
    const grid = document.createElement("div");
    grid.className = "date-picker-grid";

    // Footer with selected date + actions
    const footer = document.createElement("div");
    footer.className = "date-picker-footer";

    const selectedLabel = document.createElement("span");
    selectedLabel.className = "date-picker-selected";

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
    footer.appendChild(selectedLabel);
    footer.appendChild(actions);

    panel.appendChild(header);
    panel.appendChild(dowRow);
    panel.appendChild(grid);
    panel.appendChild(footer);
    overlay.appendChild(panel);
    document.body.appendChild(overlay);

    function clampDay() {
      const max = daysInMonth(year, month);
      if (day > max) day = max;
      if (day < 1) day = 1;
    }

    function render() {
      monthLabel.textContent = `${monthNamesLong[month]} ${year}`;
      selectedLabel.textContent = `${year}-${pad2(month + 1)}-${pad2(day)}`;

      grid.innerHTML = "";
      const startDow = startDayOfWeek(year, month);
      const maxDays = daysInMonth(year, month);
      const today = new Date();
      const isCurrentMonth =
        today.getFullYear() === year && today.getMonth() === month;
      const todayDay = today.getDate();

      // Empty leading cells
      for (let i = 0; i < startDow; i++) {
        const empty = document.createElement("span");
        empty.className = "date-picker-cell date-picker-cell-empty";
        grid.appendChild(empty);
      }

      for (let d = 1; d <= maxDays; d++) {
        const cell = document.createElement("button");
        cell.className = "date-picker-cell";
        cell.textContent = `${d}`;
        if (d === day) cell.classList.add("date-picker-cell-selected");
        if (isCurrentMonth && d === todayDay)
          cell.classList.add("date-picker-cell-today");
        const dayVal = d;
        cell.addEventListener("click", () => {
          day = dayVal;
          render();
        });
        cell.addEventListener("dblclick", () => {
          day = dayVal;
          confirm();
        });
        grid.appendChild(cell);
      }
    }

    function prevMonth() {
      if (month === 0) {
        month = 11;
        year--;
      } else {
        month--;
      }
      clampDay();
      render();
    }

    function nextMonth() {
      if (month === 11) {
        month = 0;
        year++;
      } else {
        month++;
      }
      clampDay();
      render();
    }

    const close = (result: string | null) => {
      overlay.remove();
      document.removeEventListener("keydown", onKeyDown);
      resolve(result);
    };

    function confirm() {
      const parsed = new Date(year, month, day);
      close(formatDateByPattern(parsed, format));
    }

    prevBtn.addEventListener("click", prevMonth);
    nextBtn.addEventListener("click", nextMonth);
    cancelBtn.addEventListener("click", () => close(null));
    insertBtn.addEventListener("click", confirm);

    overlay.addEventListener("mousedown", (e) => {
      if (e.target === overlay) close(null);
    });

    function onKeyDown(e: KeyboardEvent) {
      switch (e.key) {
        case "Escape":
          e.preventDefault();
          close(null);
          break;
        case "Enter":
          e.preventDefault();
          confirm();
          break;
        case "ArrowLeft":
          e.preventDefault();
          if (day > 1) {
            day--;
          } else {
            prevMonth();
            day = daysInMonth(year, month);
          }
          render();
          break;
        case "ArrowRight":
          e.preventDefault();
          if (day < daysInMonth(year, month)) {
            day++;
          } else {
            nextMonth();
            day = 1;
          }
          render();
          break;
        case "ArrowUp":
          e.preventDefault();
          if (day > 7) {
            day -= 7;
          } else {
            prevMonth();
            day = Math.min(day, daysInMonth(year, month));
          }
          render();
          break;
        case "ArrowDown":
          e.preventDefault();
          if (day + 7 <= daysInMonth(year, month)) {
            day += 7;
          } else {
            nextMonth();
            day = Math.min(day, daysInMonth(year, month));
          }
          render();
          break;
      }
    }

    document.addEventListener("keydown", onKeyDown);
    render();
    window.setTimeout(() => panel.focus(), 0);
  });
}
