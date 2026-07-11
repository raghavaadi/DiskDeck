on breadcrumbSignature(appGroup)
    tell application "System Events"
        set backControl to button "← Back" of appGroup
        set backPosition to position of backControl
        set backX to item 1 of backPosition
        set backY to item 2 of backPosition
        set signatureParts to {}
        repeat with elementRef in UI elements of appGroup
            try
                set elementRole to role of elementRef
                set elementName to name of elementRef as text
                set elementPosition to position of elementRef
                set elementX to item 1 of elementPosition
                set elementY to item 2 of elementPosition
                if (elementRole is "AXButton" or elementRole is "AXStaticText") and elementX < backX and elementY ≥ (backY - 8) and elementY ≤ (backY + 8) then
                    set end of signatureParts to elementName
                end if
            end try
        end repeat
    end tell
    set AppleScript's text item delimiters to "|"
    set signatureText to signatureParts as text
    set AppleScript's text item delimiters to ""
    return signatureText
end breadcrumbSignature

on menuIsVisible(appGroup)
    tell application "System Events"
        set elementNames to name of every UI element of appGroup
    end tell
    set sawOpen to false
    set sawReveal to false
    set sawMove to false
    repeat with elementName in elementNames
        try
            set labelText to elementName as text
            if labelText is "Open" then set sawOpen to true
            if labelText is "Reveal in Finder" then set sawReveal to true
            if labelText starts with "Move to SSD" then set sawMove to true
        end try
    end repeat
    return sawOpen and sawReveal and sawMove
end menuIsVisible

on largestTileCenter(appGroup)
    set bestArea to 0
    set bestX to 0
    set bestY to 0
    tell application "System Events"
        repeat with elementRef in UI elements of appGroup
            try
                if role of elementRef is "AXUnknown" then
                    set elementSize to size of elementRef
                    set elementWidth to item 1 of elementSize
                    set elementHeight to item 2 of elementSize
                    set elementArea to elementWidth * elementHeight
                    if elementWidth ≥ 200 and elementHeight ≥ 200 and elementArea > bestArea then
                        set elementPosition to position of elementRef
                        set bestArea to elementArea
                        set bestX to (item 1 of elementPosition) + (elementWidth / 2)
                        set bestY to (item 2 of elementPosition) + (elementHeight / 2)
                    end if
                end if
            end try
        end repeat
    end tell
    if bestArea is 0 then error "No treemap item is available. Finish a scan first." number 1
    return ((round bestX) as text) & " " & ((round bestY) as text)
end largestTileCenter

on run argv
    set commandName to "check"
    if (count of argv) > 0 then set commandName to item 1 of argv

    tell application "System Events"
        if not UI elements enabled then error "Accessibility is required. Enable the terminal or Codex in System Settings → Privacy & Security → Accessibility." number 1
    end tell

    tell application "DiskDeck" to activate

    tell application "System Events"
        if not (exists process "DiskDeck") then error "DiskDeck is not running." number 1
        tell process "DiskDeck"
            set frontmost to true
            repeat with attempt from 1 to 50
                if (exists window 1) and (exists group 1 of window 1) then exit repeat
                delay 0.1
            end repeat
            if not (exists window 1) then error "DiskDeck has no visible window." number 1
            if not (exists group 1 of window 1) then error "DiskDeck content controls did not become available." number 1
            set appGroup to group 1 of window 1
            if not (exists static text "DiskDeck" of appGroup) then error "DiskDeck title control is unavailable." number 1
            if not (exists button "← Back" of appGroup) then error "DiskDeck Back control is unavailable." number 1

            if commandName is "check" then
                return "PASS: signed UI controls available"
            else if commandName is "signature" then
                return my breadcrumbSignature(appGroup)
            else if commandName is "tile-center" then
                return my largestTileCenter(appGroup)
            else if commandName is "menu-visible" then
                return my menuIsVisible(appGroup)
            else if commandName is "escape" then
                key code 53
                return "PASS: Escape sent"
            else if commandName is "back" then
                set beforeSignature to my breadcrumbSignature(appGroup)
                if beforeSignature does not contain "|/|" then return "SKIP: already at Data root"
                click button "← Back" of appGroup
                delay 0.5
                set afterSignature to my breadcrumbSignature(appGroup)
                if afterSignature is beforeSignature then error "Back did not change the breadcrumb." number 1
                return "PASS: Back navigation"
            else
                error "Usage: ui-smoke.applescript check|signature|tile-center|menu-visible|escape|back" number 1
            end if
        end tell
    end tell
end run
