import os
import json
import argparse
import sys
import re

# ==========================================
#               CONFIGURATION
# ==========================================
CONFIG = {
    "root_directory": "UnpackedBDAT",
    # Xenoblade 2 typically stores the main text in the "name" field in JSON exports
    "target_key": "name",
    "log_file": "text_balancing_log.txt",
    "error_file": "text_overflow_errors.txt",

    # MAPPING STYLE IDs TO PROFILES
    # 82 = Cinematic (Event rules)
    # 84 = Standard Bubble (NPC rules)
    # 62 = Standard Bubble (NPC rules)
    # 165 = Player Choice/Bubble (NPC rules)
    "style_overrides": {
        82: "event",
        84: "npc",
        62: "npc",
        165: "npc"
    },

    # DEFINING RULES BASED ON GUI REFERENCE
    "profiles": {
        "event": {
            "name": "Event (Cinematic)",
            # Files/Folders starting with these
            "prefixes": ["bf"],
            "max_lines": 2,
            # GUI Limit: 54 chars
            "absolute_max_width": 55,
            # Auto-calculation for splitting points roughly based on width
            "split_threshold_for_2": 40,
            "split_threshold_for_3": 108000 # Effectively infinite, prevents 3 lines
        },
        "npc": {
            "name": "NPC/Quest (Bubble)",
            # Files/Folders starting with these (from GUI check_line_length)
            "prefixes": ["campfev", "fev", "kizuna", "qst", "tlk"],
            "max_lines": 3,
            # GUI Limit: 39 chars
            "absolute_max_width": 39,
            # Auto-calculation for splitting points
            "split_threshold_for_2": 39,
            "split_threshold_for_3": 70
        }
    }
}

# ==========================================
#            CORE LOGIC
# ==========================================

def get_profile_for_path(file_path):
    """
    Determines default profile based on filename or parent folder.
    This is used as a fallback if no specific 'style' ID is found in the row.
    """
    norm_path = file_path.replace("\\", "/")
    filename = os.path.basename(norm_path).lower()
    parent_dir = os.path.basename(os.path.dirname(norm_path)).lower()

    # Check Event Profile Prefixes
    for prefix in CONFIG["profiles"]["event"]["prefixes"]:
        if filename.startswith(prefix) or parent_dir.startswith(prefix):
            return CONFIG["profiles"]["event"]

    # Check NPC Profile Prefixes
    for prefix in CONFIG["profiles"]["npc"]["prefixes"]:
        if filename.startswith(prefix) or parent_dir.startswith(prefix):
            return CONFIG["profiles"]["npc"]

    return None

def clean_and_flatten(text):
    """Removes existing newlines and collapses multiple spaces."""
    if not text: return ""
    # Convert literal \n from JSON strings if present, then actual newlines
    flat = text.replace('\\n', ' ').replace('\n', ' ')
    flat = flat.replace('\r', '')
    return " ".join(flat.split())

def get_visual_length(text):
    """
    Calculates character count ignoring control tags (Square Brackets).
    """
    # Remove anything inside square brackets [ML:...] [System:...] etc.
    clean_text = re.sub(r'\[.*?\]', '', text)
    clean_text = clean_text.replace('\u200B', '')
    return len(clean_text)

def tokenize_keeping_tags_intact(text):
    """
    Splits text by spaces, but ensures spaces INSIDE tags don't split the block.
    """
    def protect_match(match):
        return match.group(0).replace(' ', '<<SPACE>>')

    # Protect content inside any [...] tags
    protected_text = re.sub(r'\[.*?\]', protect_match, text)

    raw_words = protected_text.split(' ')
    words = [w.replace('<<SPACE>>', ' ') for w in raw_words if w]
    return words

def force_split(words, num_lines):
    """Strictly splits 'words' into exactly 'num_lines' based on VISUAL length."""
    if num_lines <= 1:
        return [" ".join(words)]

    total_visual_len = sum(get_visual_length(w) for w in words) + (len(words) - 1)
    target_visual_len = total_visual_len / num_lines

    lines = []
    current_words = words[:]

    for _ in range(num_lines - 1):
        best_split = 0
        best_diff = float('inf')
        current_visual_len = 0

        for i, w in enumerate(current_words):
            w_vis_len = get_visual_length(w)
            len_with = current_visual_len + w_vis_len + (1 if current_visual_len > 0 else 0)
            diff = abs(len_with - target_visual_len)

            if diff <= best_diff:
                best_diff = diff
                best_split = i + 1
                current_visual_len = len_with
            else:
                break

        lines.append(" ".join(current_words[:best_split]))
        current_words = current_words[best_split:]
        if not current_words: break

    if current_words:
        lines.append(" ".join(current_words))
    return lines

def process_text(text_content, profile):
    if not isinstance(text_content, str) or not text_content.strip():
        return text_content

    clean_text = clean_and_flatten(text_content)
    words = tokenize_keeping_tags_intact(clean_text)

    # Calculate total visual length to decide how many lines needed
    total_len = sum(get_visual_length(w) for w in words) + (len(words) - 1)

    # Logic to determine required lines based on limits
    if total_len <= profile["split_threshold_for_2"]:
        target_lines = 1
    elif total_len <= profile["split_threshold_for_3"] and profile["max_lines"] >= 2:
        target_lines = 2
    else:
        target_lines = 3 if profile["max_lines"] >= 3 else 2

    final_lines = force_split(words, target_lines)
    return "\n".join(final_lines)

def check_for_overflow(text_block, max_width):
    lines = text_block.split('\n')
    for line in lines:
        vis_len = get_visual_length(line)
        if vis_len > max_width:
            return True, vis_len
    return False, 0

# ==========================================
#           FILE PROCESSING
# ==========================================

def process_single_file(file_path, log, err_log, stats, forced_profile=None):
    # Determine the file-level default profile
    file_default_profile = get_profile_for_path(file_path)

    # If we aren't forcing a CLI profile and can't find a file match, return
    if not forced_profile and not file_default_profile:
        return

    try:
        with open(file_path, 'r', encoding='utf-8') as f:
            data = json.load(f)

        modified = False

        if "rows" in data:
            for row in data["rows"]:
                target_key = CONFIG["target_key"]

                # Fallback: check if the key exists, if not, try standard 'name' or row ID
                if target_key not in row and "<DBAF43F0>" in row:
                     target_key = "<DBAF43F0>"

                if target_key in row:
                    original_text = row[target_key]
                    if not original_text or original_text == "":
                        continue

                    # ---------------------------------------------------------
                    # PROFILE SELECTION LOGIC (ROW LEVEL)
                    # ---------------------------------------------------------
                    active_profile = None

                    # 1. If CLI forced a mode (-mode 1/2), it overrides everything
                    if forced_profile:
                        active_profile = forced_profile
                    else:
                        # 2. Check for specific "style" ID in the row
                        # 84, 62, 165 -> NPC | 82 -> Event
                        row_style = row.get("style")

                        # Ensure row_style is an int if possible
                        if row_style is not None and isinstance(row_style, int):
                            if row_style in CONFIG["style_overrides"]:
                                profile_key = CONFIG["style_overrides"][row_style]
                                active_profile = CONFIG["profiles"][profile_key]

                        # 3. Fallback to File-level default
                        if active_profile is None:
                            active_profile = file_default_profile

                    # If we still have no profile (e.g. unknown file prefix and no style ID), skip
                    if active_profile is None:
                        continue

                    # ---------------------------------------------------------
                    # PROCESSING
                    # ---------------------------------------------------------
                    new_text = process_text(original_text, active_profile)

                    is_overflow, max_len = check_for_overflow(new_text, active_profile["absolute_max_width"])
                    if is_overflow:
                        log_error(err_log, file_path, row.get("$id", "?"), new_text, max_len, active_profile["absolute_max_width"])
                        stats['errors'] += 1

                    if original_text != new_text:
                        row[target_key] = new_text
                        log_change(log, file_path, row.get("$id", "?"), original_text, new_text)
                        modified = True
                        stats['changes'] += 1

        if modified:
            with open(file_path, 'w', encoding='utf-8') as f:
                json.dump(data, f, indent=2, ensure_ascii=False)
            stats['files_processed'] += 1

    except Exception as e:
        print(f"Error processing {file_path}: {e}")

def log_change(logfile, filename, row_id, old_text, new_text):
    logfile.write(f"FILE: {filename} | ID: {row_id}\n")
    logfile.write("-" * 60 + "\n")
    vis_len = get_visual_length(clean_and_flatten(old_text))
    logfile.write(f"OLD (Vis Len: {vis_len}):\n{old_text}\n")
    logfile.write(f"\nNEW ({new_text.count(chr(10)) + 1} lines):\n{new_text}\n")
    logfile.write("-" * 60 + "\n\n")

def log_error(errfile, filename, row_id, text, max_len, limit):
    errfile.write(f"OVERFLOW: {filename} | ID: {row_id}\n")
    errfile.write(f"Visual Width: {max_len} (Limit: {limit})\n")
    errfile.write("-" * 60 + "\n")
    errfile.write(f"{text}\n")
    errfile.write("-" * 60 + "\n\n")

# ==========================================
#               MAIN ENTRY
# ==========================================

def main():
    parser = argparse.ArgumentParser(
        description="""
==============================================================================
             XENOBLADE 2 TEXT AUTO-BALANCER TOOL
==============================================================================
Adapts text to Xenoblade 2 line limits.
Targets JSON field: 'name' (or '<DBAF43F0>' if 'name' is missing).

LOGIC PRIORITY:
1. CLI Forced Mode (-mode)
2. Row "style" ID (82=Cinematic, 84/62/165=Bubble)
3. Filename/Folder prefix (bf=Cinematic, qst/tlk=Bubble)

PROFILES:
1. Event Mode (bf / Style 82):
   - Max 55 chars/line, Max 2 lines.

2. NPC/Quest Mode (qst, tlk / Style 84, 62, 165):
   - Max 39 chars/line, Max 3 lines.
""",
        formatter_class=argparse.RawTextHelpFormatter
    )

    parser.add_argument(
        "-single",
        metavar="FILE_PATH",
        help="Process only this specific JSON file."
    )

    parser.add_argument(
        "-mode",
        type=int,
        choices=[1, 2],
        help="""Force a specific logic mode (Overrides Style IDs):
  1 = Event Mode (Max 55 chars)
  2 = NPC/Quest Mode (Max 39 chars)"""
    )

    args = parser.parse_args()

    stats = {'files_processed': 0, 'changes': 0, 'errors': 0}

    # Determine if we are forcing a profile
    forced_profile = None
    if args.mode == 1:
        forced_profile = CONFIG["profiles"]["event"]
        print(f"FORCED MODE: Using {forced_profile['name']} logic (Ignoring Style IDs).")
    elif args.mode == 2:
        forced_profile = CONFIG["profiles"]["npc"]
        print(f"FORCED MODE: Using {forced_profile['name']} logic (Ignoring Style IDs).")

    print("\nStarting XB2 Text Balancer...")

    with open(CONFIG["log_file"], "w", encoding="utf-8") as log, \
         open(CONFIG["error_file"], "w", encoding="utf-8") as err_log:

        # CASE 1: Single File Mode
        if args.single:
            print(f"Targeting Single File: {args.single}")
            if os.path.exists(args.single):
                # Use forced profile if provided, otherwise detect
                # Note: process_single_file now handles the style fallback logic internally
                process_single_file(args.single, log, err_log, stats, forced_profile)
            else:
                print(f"Error: File not found -> {args.single}")

        # CASE 2: Batch Directory Mode
        else:
            print(f"Scanning Directory: {CONFIG['root_directory']}")
            for root, dirs, files in os.walk(CONFIG["root_directory"]):
                for file in files:
                    if file.endswith(".json"):
                        process_single_file(os.path.join(root, file), log, err_log, stats, forced_profile)

    print("\n" + "="*40)
    print(f"Processing Complete.")
    print(f"Files Modified:    {stats['files_processed']}")
    print(f"Text Rows Updated: {stats['changes']}")
    print(f"Overflow Errors:   {stats['errors']}")
    print(f"Logs saved to:     {CONFIG['log_file']}")
    print("="*40 + "\n")

if __name__ == "__main__":
    main()
