-- What every function of the registry checks first. Included by names.lua.

local MAX_NAME = 32

-- Refuses anything but 1 to MAX_NAME lowercase letters, digits and dashes.
local function check_name(name)
    if type(name) ~= "string" or #name == 0 or #name > MAX_NAME or not string.match(name, "^[a-z0-9%-]+$") then
        error("a name is 1 to " .. MAX_NAME .. " lowercase letters, digits or dashes")
    end
end
