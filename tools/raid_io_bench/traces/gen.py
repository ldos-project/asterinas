
# Part of LAKE: Towards a Machine Learning-Assisted Kernel with LAKE
# Copyright (C) 2022-2024 Henrique Fingler
# Copyright (C) 2022-2024 Isha Tarte
# 
# This program is free software: you can redistribute it and/or modify
# it under the terms of the GNU General Public License as published by
# the Free Software Foundation, either version 3 of the License, or
# (at your option) any later version.
# 
# This program is distributed in the hope that it will be useful,
# but WITHOUT ANY WARRANTY; without even the implied warranty of
# MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the
# GNU General Public License for more details.
# 
# You should have received a copy of the GNU General Public License
# along with this program.  If not, see <https://www.gnu.org/licenses/>.


import math
import numpy as np
import sys

# Changed from LAKE: <max offset in GB> and <seed> are arguments (the offset was a constant, and the
# seed makes a trace reproducible); sizes are aligned to 4 KiB rather than 512 B; the unused
# matplotlib/scipy/statistics imports are gone so that only numpy is needed.
if len(sys.argv) != 9:
    print("Need argument: <file output> <readpct e.g. 0.7> <total time in sec> avg_read/max avg_write/min/max/stdev <arrival rate in us> <max offset in GB> <seed>")
    sys.exit(1)

KB = 1024
MB = 1024*1024
GB = 1024*1024*1024
S_TO_US = 1000*1000

#configs
MAX_BYTE_OFFSET = int(sys.argv[7])*GB
rng = np.random.RandomState(int(sys.argv[8]))
READ_PCT = float(sys.argv[2])
TIME_US = int(sys.argv[3]) *S_TO_US  #seconds times us

avg_rd, max_rd = [int(x)*KB for x in sys.argv[4].split("/")]
avg_wt, max_wt = [int(x)*KB for x in sys.argv[5].split("/")]

print(f"rd avg {avg_rd}")
print(f"wt avg {avg_wt}")
stdev_rd =  (math.log(max_rd) - math.log(avg_rd))/3
stdev_wt =  (math.log(max_wt) - math.log(avg_wt))/3

ARRIVAL_RATE_US = float(sys.argv[6])

def get_next_multiple(A, B):
    if (A % B):
        A = A + (B - A % B)
    return A

max_size = 0
step_size = 300
total_time = 0
done = False
with open(sys.argv[1], "w") as fp:
    while not done:
        timestamps_us = rng.exponential(ARRIVAL_RATE_US, step_size)
        
        read_sizes = rng.lognormal(math.log(avg_rd), stdev_rd, step_size)
        write_sizes = rng.lognormal(math.log(avg_wt), stdev_wt, step_size)
        read_sizes[read_sizes > max_rd] = max_rd
        write_sizes[write_sizes > max_wt] = max_wt

        offsets = rng.randint(0, MAX_BYTE_OFFSET, size=step_size)
        ops = rng.choice([0, 1], size=step_size, p=[READ_PCT, 1-READ_PCT])

        for i in range(step_size):
            aligned_offset = get_next_multiple(abs(offsets[i]), 4096)
            aligned_offset = min(aligned_offset, MAX_BYTE_OFFSET)
            aligned_offset = max(aligned_offset, 128*MB) #dont write to lower offsets

            if ops[i] == 0:
                aligned_size = get_next_multiple(int(read_sizes[i]), 4096)
            else:
                aligned_size = get_next_multiple(int(write_sizes[i]), 4096)

            line = f"{total_time:.5f} 0 {int(aligned_offset)} {int(aligned_size)} {ops[i]}\n"
            fp.write(line)
            
            total_time += timestamps_us[i]
            if total_time >= TIME_US:
                done = True
                break




