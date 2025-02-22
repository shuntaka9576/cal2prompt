Output ./docs/gif/use-case-mcp-insert_calendar_event.gif
Set Theme "Dracula"

Set FontSize 24
Set Width 1200
Set Height 500
Set Framerate 60

Type 'cal2prompt mcp'
Sleep 3s
Enter

Sleep 3s

Type '{"jsonrpc":"2.0","method":"tools/call","id":1,"params":{"name": "insert_calendar_event", "arguments": {"summary": "test", "start":"2025-02-22 10:00", "end": "2025-02-22 11:00"}}}'
Sleep 3s
Enter

Sleep 5s
