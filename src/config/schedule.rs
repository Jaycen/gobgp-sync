use chrono::{DateTime, Datelike, Duration, Local, Months, NaiveDate, NaiveTime, Weekday};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SyncUnit {
    Day,
    Week,
    Month,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SyncSchedule {
    pub every: u32,
    pub unit: SyncUnit,
    pub time: NaiveTime,
    pub weekdays: Vec<Weekday>,
    pub month_days: Vec<u32>,
}

impl Default for SyncSchedule {
    fn default() -> Self {
        Self {
            every: 1,
            unit: SyncUnit::Day,
            time: NaiveTime::from_hms_opt(2, 0, 0).unwrap(),
            weekdays: Vec::new(),
            month_days: Vec::new(),
        }
    }
}

impl SyncSchedule {
    pub fn parse(raw: &str) -> Self {
        let raw = raw.trim();
        if raw.is_empty() {
            log::warn!("empty sync_time, using 1d 02:00");
            return Self::default();
        }

        if let Ok(time) = NaiveTime::parse_from_str(raw, "%H:%M") {
            return Self {
                time,
                ..Self::default()
            };
        }

        let mut time = None;
        let mut every = 1u32;
        let mut unit = None;
        let mut weekdays = Vec::new();
        let mut month_days = Vec::new();
        let mut unknown = false;

        for tok in raw.split(|c: char| c.is_whitespace() || c == '/') {
            let tok = tok.trim();
            if tok.is_empty() || tok.eq_ignore_ascii_case("every") {
                continue;
            }
            if let Ok(t) = NaiveTime::parse_from_str(tok, "%H:%M") {
                time = Some(t);
                continue;
            }
            if let Some((n, u)) = parse_interval(tok) {
                every = n.max(1);
                unit = Some(u);
                continue;
            }
            if let Some(days) = parse_weekdays(tok) {
                weekdays.extend(days);
                continue;
            }
            if let Some(days) = parse_month_days(tok) {
                month_days.extend(days);
                continue;
            }
            log::warn!("ignored sync_time token: {}", tok);
            unknown = true;
        }

        weekdays.sort_by_key(|w| w.num_days_from_monday());
        weekdays.dedup();
        month_days.sort_unstable();
        month_days.dedup();

        let time = time.unwrap_or_else(|| NaiveTime::from_hms_opt(2, 0, 0).unwrap());
        let unit = unit.unwrap_or(if !weekdays.is_empty() {
            SyncUnit::Week
        } else if !month_days.is_empty() {
            SyncUnit::Month
        } else {
            SyncUnit::Day
        });

        if unknown && time == NaiveTime::from_hms_opt(2, 0, 0).unwrap() && unit == SyncUnit::Day {
            log::warn!("invalid sync_time '{}', using 1d 02:00", raw);
        }

        Self {
            every,
            unit,
            time,
            weekdays,
            month_days,
        }
    }

    pub fn describe(&self) -> String {
        let unit = match self.unit {
            SyncUnit::Day => "day",
            SyncUnit::Week => "week",
            SyncUnit::Month => "month",
        };
        let mut s = format!(
            "every {} {} at {}",
            self.every,
            unit,
            self.time.format("%H:%M")
        );
        if !self.weekdays.is_empty() {
            let days = self
                .weekdays
                .iter()
                .map(|w| weekday_name(*w))
                .collect::<Vec<_>>()
                .join(",");
            s.push_str(&format!(" on {days}"));
        }
        if !self.month_days.is_empty() {
            let days = self
                .month_days
                .iter()
                .map(|d| d.to_string())
                .collect::<Vec<_>>()
                .join(",");
            s.push_str(&format!(" on day {days}"));
        }
        s
    }

    // 上次拉取日仍在当前周期内，则复用快照/geo 缓存
    pub fn lifecycle_ok(&self, fetched: NaiveDate, today: NaiveDate) -> bool {
        match self.next_run_date_after(fetched) {
            Some(next) => today < next,
            None => fetched == today,
        }
    }

    pub fn next_run(&self, now: DateTime<Local>, last: Option<NaiveDate>) -> DateTime<Local> {
        let mut d = now.date_naive();
        for _ in 0..400 {
            if self.is_run_day(d, last) {
                if let Some(dt) = at_local(d, self.time) {
                    if dt > now {
                        return dt;
                    }
                }
            }
            match d.succ_opt() {
                Some(next) => d = next,
                None => break,
            }
        }
        now + Duration::days(1)
    }

    fn next_run_date_after(&self, last: NaiveDate) -> Option<NaiveDate> {
        let mut d = last.succ_opt()?;
        for _ in 0..400 {
            if self.is_run_day(d, Some(last)) {
                return Some(d);
            }
            d = d.succ_opt()?;
        }
        None
    }

    fn is_run_day(&self, d: NaiveDate, last: Option<NaiveDate>) -> bool {
        if !self.weekdays.is_empty() && !self.weekdays.contains(&d.weekday()) {
            return false;
        }
        if !self.month_days.is_empty() && !self.month_days.contains(&d.day()) {
            return false;
        }

        match last {
            None => true,
            Some(last) => {
                if self.is_plain_daily() {
                    return true;
                }
                if d <= last {
                    return false;
                }
                match self.unit {
                    SyncUnit::Day => {
                        let diff = (d - last).num_days();
                        diff >= self.every as i64 && diff % self.every as i64 == 0
                    }
                    SyncUnit::Week => {
                        let min = self.every as i64 * 7;
                        let diff = (d - last).num_days();
                        if self.weekdays.is_empty() {
                            diff >= min && diff % min == 0
                        } else if self.every <= 1 {
                            true
                        } else {
                            diff >= min
                        }
                    }
                    SyncUnit::Month => {
                        if !self.month_days.is_empty() {
                            months_between(last, d) >= self.every as i32
                        } else {
                            is_month_step(last, d, self.every)
                        }
                    }
                }
            }
        }
    }

    fn is_plain_daily(&self) -> bool {
        self.unit == SyncUnit::Day
            && self.every <= 1
            && self.weekdays.is_empty()
            && self.month_days.is_empty()
    }
}

fn at_local(date: NaiveDate, time: NaiveTime) -> Option<DateTime<Local>> {
    match date.and_time(time).and_local_timezone(Local) {
        chrono::LocalResult::Single(dt) => Some(dt),
        chrono::LocalResult::Ambiguous(a, _) => Some(a),
        chrono::LocalResult::None => date
            .succ_opt()
            .and_then(|next| next.and_time(time).and_local_timezone(Local).earliest()),
    }
}

fn months_between(from: NaiveDate, to: NaiveDate) -> i32 {
    (to.year() * 12 + to.month() as i32) - (from.year() * 12 + from.month() as i32)
}

fn is_month_step(last: NaiveDate, d: NaiveDate, every: u32) -> bool {
    let mut t = last;
    for _ in 0..36 {
        t = match t.checked_add_months(Months::new(every)) {
            Some(v) => v,
            None => return false,
        };
        if t == d {
            return true;
        }
        if t > d {
            return false;
        }
    }
    false
}

fn parse_interval(tok: &str) -> Option<(u32, SyncUnit)> {
    let tok = tok.to_ascii_lowercase();
    let (n, rest) = tok.split_at(tok.find(|c: char| !c.is_ascii_digit()).unwrap_or(tok.len()));
    if n.is_empty() {
        return None;
    }
    let every: u32 = n.parse().ok()?;
    let unit = match rest {
        "d" | "day" | "days" => SyncUnit::Day,
        "w" | "week" | "weeks" => SyncUnit::Week,
        "m" | "mo" | "month" | "months" => SyncUnit::Month,
        _ => return None,
    };
    Some((every.max(1), unit))
}

fn parse_weekdays(tok: &str) -> Option<Vec<Weekday>> {
    let mut out = Vec::new();
    for part in tok.split(',') {
        let part = part.trim();
        if part.is_empty() {
            continue;
        }
        if let Some((a, b)) = part.split_once('-') {
            let start = parse_weekday(a.trim())?;
            let end = parse_weekday(b.trim())?;
            let mut i = start.num_days_from_monday();
            let end_i = end.num_days_from_monday();
            if end_i < i {
                return None;
            }
            while i <= end_i {
                out.push(weekday_from_monday(i)?);
                i += 1;
            }
        } else {
            out.push(parse_weekday(part)?);
        }
    }
    if out.is_empty() {
        None
    } else {
        Some(out)
    }
}

fn parse_weekday(s: &str) -> Option<Weekday> {
    match s.to_ascii_lowercase().as_str() {
        "mon" | "monday" => Some(Weekday::Mon),
        "tue" | "tues" | "tuesday" => Some(Weekday::Tue),
        "wed" | "wednesday" => Some(Weekday::Wed),
        "thu" | "thur" | "thurs" | "thursday" => Some(Weekday::Thu),
        "fri" | "friday" => Some(Weekday::Fri),
        "sat" | "saturday" => Some(Weekday::Sat),
        "sun" | "sunday" => Some(Weekday::Sun),
        _ => None,
    }
}

fn weekday_from_monday(i: u32) -> Option<Weekday> {
    match i {
        0 => Some(Weekday::Mon),
        1 => Some(Weekday::Tue),
        2 => Some(Weekday::Wed),
        3 => Some(Weekday::Thu),
        4 => Some(Weekday::Fri),
        5 => Some(Weekday::Sat),
        6 => Some(Weekday::Sun),
        _ => None,
    }
}

fn weekday_name(w: Weekday) -> &'static str {
    match w {
        Weekday::Mon => "Mon",
        Weekday::Tue => "Tue",
        Weekday::Wed => "Wed",
        Weekday::Thu => "Thu",
        Weekday::Fri => "Fri",
        Weekday::Sat => "Sat",
        Weekday::Sun => "Sun",
    }
}

fn parse_month_days(tok: &str) -> Option<Vec<u32>> {
    let mut out = Vec::new();
    for part in tok.split(',') {
        let part = part.trim();
        if part.is_empty() {
            continue;
        }
        if let Some((a, b)) = part.split_once('-') {
            let start: u32 = a.trim().parse().ok()?;
            let end: u32 = b.trim().parse().ok()?;
            if start < 1 || end > 31 || start > end {
                return None;
            }
            out.extend(start..=end);
        } else {
            let day: u32 = part.parse().ok()?;
            if !(1..=31).contains(&day) {
                return None;
            }
            out.push(day);
        }
    }
    if out.is_empty() {
        None
    } else {
        Some(out)
    }
}
