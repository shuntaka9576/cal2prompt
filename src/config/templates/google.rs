pub const STANDARD: &str = r#"Here is your schedule summary. Please find the details below:
{% for day in days %}
## Date: {{ day.date }}

{% if day.all_day_events|length > 0 %}
### All-Day Events:
{% for ev in day.all_day_events %}
- {{ ev.summary }}
  - (All Day)
  - Location: {{ ev.location or "N/A" }}
  - Description: {{ ev.description or "No description." }}
  - Attendees:
    {% if ev.attendees|length > 0 %}
      {% for a in ev.attendees %}
      - {{ a }}
      {% endfor %}
    {% else %}
    - (No attendees)
    {% endif %}
{% endfor %}
{% endif %}

### Events:
{% if day.timed_events|length == 0 %}
(No timed events)
{% else %}
{% for ev in day.timed_events %}
- {{ ev.summary }}
  - Start: {{ ev.start }}
  - End:   {{ ev.end }}
  - Location: {{ ev.location or "N/A" }}
  - Description: {{ ev.description or "No description." }}
  - Attendees:
    {% if ev.attendees|length > 0 %}
      {% for a in ev.attendees %}
      - {{ a }}
      {% endfor %}
    {% else %}
    - (No attendees)
    {% endif %}
{% endfor %}
{% endif %}
{% endfor %}
"#;
