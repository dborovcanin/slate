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

function clamp(value: number, min: number, max: number): number {
  return Math.min(max, Math.max(min, value));
}

export function formatDateByPattern(date: Date, pattern: string): string {
  const yyyy = `${date.getFullYear()}`;
  const yy = yyyy.slice(-2);
  const mm = pad2(date.getMonth() + 1);
  const m = `${date.getMonth() + 1}`;
  const dd = pad2(date.getDate());
  const d = `${date.getDate()}`;
  const HH = pad2(date.getHours());
  const H = `${date.getHours()}`;
  const Min = pad2(date.getMinutes());
  const MMM = monthNamesShort[date.getMonth()];
  const MMMM = monthNamesLong[date.getMonth()];

  const out = pattern || "%Y-%m-%d";

  if (out.includes("%")) {
    const replacements: [string, string][] = [
      ["%Y", yyyy],
      ["%y", yy],
      ["%m", mm],
      ["%d", dd],
      ["%H", HH],
      ["%M", Min],
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
    ["HH", HH],
    ["mm", Min],
    ["YY", yy],
    ["M", m],
    ["D", d],
    ["H", H],
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

export interface DateTimePickerOptions {
  dateFormat: string;
  dateTimeFormat: string;
  mode?: "date" | "remind";
  requireTime?: boolean;
}

export interface DateTimePickerResult {
  insertText: string;
  hasTime: boolean;
  remindAtMs: number;
  displayAt: string;
}

export function openDateTimePicker(
  options: DateTimePickerOptions,
): Promise<DateTimePickerResult | null> {
  const mode = options.mode ?? "date";
  const requireTime = options.requireTime ?? mode === "remind";
  const now = new Date();
  let year = now.getFullYear();
  let month = now.getMonth(); // 0-based
  let day = now.getDate();
  let includeTime = requireTime;
  let hour = now.getHours();
  let minute = now.getMinutes();
  const restoreTarget =
    document.activeElement instanceof HTMLElement ? document.activeElement : null;

  return new Promise((resolve) => {
    const overlay = document.createElement("div");
    overlay.className = "date-picker-overlay";

    const panel = document.createElement("div");
    panel.className = "date-picker-panel";
    panel.tabIndex = -1;
    panel.setAttribute("role", "dialog");
    panel.setAttribute("aria-modal", "true");
    panel.setAttribute("aria-label", mode === "remind" ? "Reminder picker" : "Date picker");

    const header = document.createElement("div");
    header.className = "date-picker-header";

    const prevBtn = document.createElement("button");
    prevBtn.className = "date-picker-nav";
    prevBtn.textContent = "‹";
    prevBtn.setAttribute("aria-label", "Previous month");

    const monthLabel = document.createElement("span");
    monthLabel.className = "date-picker-month-label";
    monthLabel.id = "date-picker-month-label";
    panel.setAttribute("aria-labelledby", monthLabel.id);

    const nextBtn = document.createElement("button");
    nextBtn.className = "date-picker-nav";
    nextBtn.textContent = "›";
    nextBtn.setAttribute("aria-label", "Next month");

    header.appendChild(prevBtn);
    header.appendChild(monthLabel);
    header.appendChild(nextBtn);

    const dowRow = document.createElement("div");
    dowRow.className = "date-picker-dow";
    for (const dh of dayHeaders) {
      const cell = document.createElement("span");
      cell.className = "date-picker-dow-cell";
      cell.textContent = dh;
      dowRow.appendChild(cell);
    }

    const grid = document.createElement("div");
    grid.className = "date-picker-grid";
    grid.setAttribute("role", "grid");

    const timeRow = document.createElement("div");
    timeRow.className = "date-picker-time-row";

    const timeLeading = document.createElement("label");
    timeLeading.className = "date-picker-time-label";

    const timeToggle = document.createElement("input");
    timeToggle.type = "checkbox";
    timeToggle.className = "date-picker-time-toggle";
    timeToggle.checked = includeTime;
    timeToggle.disabled = requireTime;

    if (requireTime) {
      timeLeading.textContent = "Time";
    } else {
      timeLeading.appendChild(timeToggle);
      const toggleText = document.createElement("span");
      toggleText.textContent = "Include time";
      timeLeading.appendChild(toggleText);
    }

    const timeFields = document.createElement("div");
    timeFields.className = "date-picker-time-fields";

    const hourInput = document.createElement("input");
    hourInput.type = "number";
    hourInput.className = "date-picker-time-input";
    hourInput.min = "0";
    hourInput.max = "23";
    hourInput.step = "1";
    hourInput.value = `${hour}`;
    hourInput.setAttribute("aria-label", "Hour");

    const colon = document.createElement("span");
    colon.className = "date-picker-time-colon";
    colon.textContent = ":";

    const minuteInput = document.createElement("input");
    minuteInput.type = "number";
    minuteInput.className = "date-picker-time-input";
    minuteInput.min = "0";
    minuteInput.max = "59";
    minuteInput.step = "1";
    minuteInput.value = `${minute}`;
    minuteInput.setAttribute("aria-label", "Minute");

    timeFields.appendChild(hourInput);
    timeFields.appendChild(colon);
    timeFields.appendChild(minuteInput);

    timeRow.appendChild(timeLeading);
    timeRow.appendChild(timeFields);

    const footer = document.createElement("div");
    footer.className = "date-picker-footer";

    const selectedLabel = document.createElement("span");
    selectedLabel.className = "date-picker-selected";

    const actions = document.createElement("div");
    actions.className = "date-picker-actions";

    const cancelBtn = document.createElement("button");
    cancelBtn.className = "date-picker-btn";
    cancelBtn.textContent = "Cancel";

    const insertBtn = document.createElement("button");
    insertBtn.className = "date-picker-btn date-picker-btn-primary";
    insertBtn.textContent = mode === "remind" ? "Set" : "Insert";

    actions.appendChild(cancelBtn);
    actions.appendChild(insertBtn);
    footer.appendChild(selectedLabel);
    footer.appendChild(actions);

    panel.appendChild(header);
    panel.appendChild(dowRow);
    panel.appendChild(grid);
    panel.appendChild(timeRow);
    panel.appendChild(footer);
    overlay.appendChild(panel);
    document.body.appendChild(overlay);

    function parseTimeInputs() {
      const nextHour = clamp(Number.parseInt(hourInput.value, 10) || 0, 0, 23);
      const nextMinute = clamp(Number.parseInt(minuteInput.value, 10) || 0, 0, 59);
      hour = nextHour;
      minute = nextMinute;
      hourInput.value = `${hour}`;
      minuteInput.value = `${minute}`;
    }

    function setTimeEnabled(enabled: boolean) {
      hourInput.disabled = !enabled;
      minuteInput.disabled = !enabled;
      timeFields.classList.toggle("date-picker-time-fields-disabled", !enabled);
    }

    function selectedDateTime() {
      parseTimeInputs();
      const selected = new Date(
        year,
        month,
        day,
        includeTime ? hour : 0,
        includeTime ? minute : 0,
        0,
        0,
      );
      const insertText = includeTime
        ? formatDateByPattern(selected, options.dateTimeFormat)
        : formatDateByPattern(selected, options.dateFormat);
      return {
        date: selected,
        insertText,
        displayAt: formatDateByPattern(
          new Date(year, month, day, hour, minute, 0, 0),
          options.dateTimeFormat,
        ),
      };
    }

    function clampDay() {
      const max = daysInMonth(year, month);
      if (day > max) day = max;
      if (day < 1) day = 1;
    }

    function render() {
      monthLabel.textContent = `${monthNamesLong[month]} ${year}`;
      selectedLabel.textContent = selectedDateTime().insertText;
      setTimeEnabled(includeTime);

      grid.replaceChildren();
      const startDow = startDayOfWeek(year, month);
      const maxDays = daysInMonth(year, month);
      const today = new Date();
      const isCurrentMonth =
        today.getFullYear() === year && today.getMonth() === month;
      const todayDay = today.getDate();

      for (let i = 0; i < startDow; i++) {
        const empty = document.createElement("span");
        empty.className = "date-picker-cell date-picker-cell-empty";
        empty.setAttribute("aria-hidden", "true");
        grid.appendChild(empty);
      }

      for (let d = 1; d <= maxDays; d++) {
        const cell = document.createElement("button");
        cell.type = "button";
        cell.className = "date-picker-cell";
        cell.textContent = `${d}`;
        cell.setAttribute("role", "gridcell");
        cell.setAttribute("aria-selected", d === day ? "true" : "false");
        if (d === day) cell.classList.add("date-picker-cell-selected");
        if (isCurrentMonth && d === todayDay) {
          cell.classList.add("date-picker-cell-today");
          cell.setAttribute("aria-current", "date");
        }
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

    const close = (result: DateTimePickerResult | null) => {
      overlay.remove();
      document.removeEventListener("keydown", onKeyDown);
      if (restoreTarget && restoreTarget.isConnected) {
        restoreTarget.focus();
      }
      resolve(result);
    };

    function confirm() {
      if (requireTime && !includeTime) return;
      const selection = selectedDateTime();
      close({
        insertText: selection.insertText,
        hasTime: includeTime,
        remindAtMs: selection.date.getTime(),
        displayAt: selection.displayAt,
      });
    }

    prevBtn.addEventListener("click", prevMonth);
    nextBtn.addEventListener("click", nextMonth);
    cancelBtn.addEventListener("click", () => close(null));
    insertBtn.addEventListener("click", confirm);
    timeToggle.addEventListener("change", () => {
      includeTime = requireTime ? true : timeToggle.checked;
      render();
    });
    hourInput.addEventListener("change", render);
    minuteInput.addEventListener("change", render);

    overlay.addEventListener("mousedown", (e) => {
      if (e.target === overlay) close(null);
    });

    function onKeyDown(e: KeyboardEvent) {
      const target = e.target as HTMLElement | null;
      const inTimeInput =
        target === hourInput ||
        target === minuteInput;

      if (e.key === "Escape") {
        e.preventDefault();
        close(null);
        return;
      }
      if (e.key === "Enter") {
        e.preventDefault();
        confirm();
        return;
      }

      if (inTimeInput) {
        return;
      }

      switch (e.key) {
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
        case "PageUp":
          e.preventDefault();
          prevMonth();
          break;
        case "PageDown":
          e.preventDefault();
          nextMonth();
          break;
        case "t":
        case "T":
          if (!requireTime) {
            e.preventDefault();
            includeTime = !includeTime;
            timeToggle.checked = includeTime;
            render();
          }
          break;
      }
    }

    document.addEventListener("keydown", onKeyDown);
    render();
    window.setTimeout(() => panel.focus(), 0);
  });
}

export async function openDatePicker(format: string): Promise<string | null> {
  const selection = await openDateTimePicker({
    dateFormat: format,
    dateTimeFormat: `${format} %H:%M`,
    mode: "date",
    requireTime: false,
  });
  return selection?.insertText ?? null;
}
