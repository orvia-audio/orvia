// Windows Installer owns shutdown through Restart Manager. Older Orvia builds
// did not register for relaunch, so remember which interactive session to resume.
function installFolder() {
    var shell = new ActiveXObject("WScript.Shell");
    return shell.RegRead("HKLM\\SOFTWARE\\Microsoft\\Windows\\CurrentVersion\\ProgramFilesDir") + "\\Orvia\\";
}

function rememberSession() {
    var wmi = GetObject("winmgmts:root\\cimv2");
    var programs = wmi.ExecQuery("SELECT * FROM Win32_Process WHERE Name='orvia.exe' OR Name='orvia-engine.exe'");
    var folder = installFolder().toLowerCase();
    var sid = Session.Property("UserSID");
    var sessions = {};
    for (var it = new Enumerator(programs); !it.atEnd(); it.moveNext()) {
        var proc = it.item();
        var name = String(proc.Name).toLowerCase();
        if (proc.SessionId <= 0 || String(proc.ExecutablePath).toLowerCase() !== folder + name) continue;
        var owner = proc.ExecMethod_("GetOwnerSid");
        if (owner.ReturnValue !== 0 || owner.Sid !== sid) continue;
        var id = Number(proc.SessionId);
        sessions[id] = sessions[id] === "window" || name === "orvia.exe" ? "window" : "engine";
    }
    var records = [];
    for (var id in sessions) records.push(id + ":" + sessions[id]);
    if (records.length) {
        var data = sid + "|" + records.join(",");
        Session.Property("ResumeOrvia") = data;
        Session.Property("RestoreOrvia") = data;
    }
    return 1;
}

function resumeSession() {
    var parts = Session.Property("CustomActionData").split("|");
    if (parts.length !== 2 || !/^S-1-[0-9-]+$/.test(parts[0])) return 3;
    var scheduler = new ActiveXObject("Schedule.Service");
    scheduler.Connect();
    var root = scheduler.GetFolder("\\");
    var sessions = parts[1].split(",");
    for (var i = 0; i < sessions.length; ++i) {
        var target = /^(\d+):(window|engine)$/.exec(sessions[i]);
        if (!target || Number(target[1]) <= 0) return 3;
        var task = scheduler.NewTask(0);
        task.Principal.UserId = parts[0];
        task.Principal.LogonType = 3; // TASK_LOGON_INTERACTIVE_TOKEN
        task.Principal.RunLevel = 0; // TASK_RUNLEVEL_LUA
        task.Settings.DisallowStartIfOnBatteries = false;
        task.Settings.StopIfGoingOnBatteries = false;
        task.Settings.ExecutionTimeLimit = "PT0S";
        var action = task.Actions.Create(0);
        action.Path = installFolder() + (target[2] === "window" ? "orvia.exe" : "orvia-engine.exe");
        var name = "Orvia-Resume-" + target[1] + "-" + new Date().getTime();
        var registered = root.RegisterTaskDefinition(name, task, 2, parts[0], null, 3); // TASK_CREATE
        try {
            registered.RunEx("", 4, Number(target[1]), ""); // TASK_RUN_USE_SESSION_ID
        } finally {
            root.DeleteTask(name, 0);
        }
    }
    return 1;
}
