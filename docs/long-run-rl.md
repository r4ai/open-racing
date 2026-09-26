# Training Racing Policies That Last a Stint

*open-racing technical report*

## Abstract

A reinforcement-learning policy that sets fast single laps can still fail a race stint. On a 5.8 km road circuit, a GT3 policy trained with our previous recipe lapped in 119.7 s, yet none of 32 cars driving 10 laps from a standing start got past lap 4. We trace the failure to the training distribution: episodes lasted about one lap and always began on fresh tyres, so the states of later laps (hot, worn tyres and flying-lap speeds) were never trained on. We change what the policy trains on and what it observes. Episodes run for several laps, starts are randomised over worn and hot tyres, and a share of episodes restart shortly before earlier crashes. The policy sees its tyre, brake and damage state, and the reward prices tyre degradation. With these changes, all 32 cars completed 10 laps without a crash or leaving the track, in 1208.2 s for the car driven without noise. An ablation that keeps every change except the state distribution completes no stint at all. We also report a case of reward hacking, where a policy cut a chicane, and the rule that removed it.

## 1 Introduction

Deep reinforcement learning (RL) now drives simulated race cars at the limit of grip [1, 2, 3]. Most published results measure single laps or short races. A real stint adds slow dynamics that single laps hide. Tyres heat up over minutes and wear over tens of minutes, the pressures rise with temperature, and every lap after the first begins at full speed. A policy must drive well in all of these states, not only in the state of a fresh car on a fresh lap.

We train with PPO [4] in open-racing, a vehicle simulator with a thermal and wear model of the tyres. Our previous recipe produced fast laps but unreliable stints. This report explains why, and describes the training method we use now. Our contributions are:

- a diagnosis of long-run failure as a train–test mismatch in the state distribution (Section 3);
- a training method that covers the states of a stint and prices tyre degradation (Section 4);
- a 10-lap evaluation, an ablation and an analysis of driving technique (Section 5).

## 2 Background

**Racing with RL.** Fuchs et al. [2] trained a policy in Gran Turismo Sport that beat human time-trial records. The reward was progress along the track, with a penalty for touching walls, and the observations included the track ahead. GT Sophy [1] raced against champions with a distributional SAC. It added an off-course penalty and masked progress while off course. It also restarted training from states just before its own mistakes ("mistake learning"), and fed tyre wear to the policy so that it could adapt as grip changed. Song et al. [3] compare RL with optimal control at the limit of performance and find that RL can optimise the task objective directly.

**Distribution shift.** A policy trained on the states of one distribution may fail on the states it reaches itself. In imitation learning, small errors compound because the learner visits states the expert never showed [5]. The same mismatch appears in RL when training episodes are shorter than the task: states that occur only late in the task are never trained on. Pardo et al. [6] discuss how time limits change what an agent learns, and how to bootstrap values at truncation.

**Choosing start states.** Resetting to chosen states steers training towards hard parts of a task. Examples are restarts from demonstration states [7], reverse curricula that start near the goal [8], and restarts from archived states in Go-Explore [9]. Randomising the start conditions covers conditions the agent cannot easily reach itself, as domain randomisation does for simulation parameters [10].

**Reward hacking.** An agent optimises the reward it is given, not the one the designer meant. It finds unintended shortcuts when they pay more [11].

## 3 Problem: why stints fail

**Setup.** The car is a GT3 model with ABS and traction control on a 5.8 km circuit. Physics runs at 1 kHz and the policy acts at 25 Hz. The tyre model tracks the temperature of the tread (three zones) and of the carcass, the hot pressure, and wear. Grip peaks at a tread temperature of 90 °C, and falls with pressure away from its optimum and with wear.

**Baseline.** Our previous recipe trained PPO for 50 minutes. Episodes started at random points and were capped at 180 s, about 1.5 laps, and every one started on fresh tyres. The policy observed the car's motion, its inputs and the track ahead, but not its tyres.

**Observation.** Driven for 10 laps from a standing start, the baseline crashed every car by lap 4. The mean tyre grip fell from 0.974 on lap 1 to 0.957 on lap 2 and 0.927 on lap 3, and the treads reached 137 °C. The first corner was reached at 272 km/h on the flying lap 2 against 244 km/h on lap 1, with the rear-left tread averaging 110 °C over the lap, and the car crashed there.

**Diagnosis.** Every state in which the car crashed lay outside the training distribution. No training episode contained a second flying lap, and none started on hot or worn tyres. The policy could not observe its tyres either, so it could not adapt when grip dropped. This is the compounding-error problem of [5]: the policy's own driving moves it into states it was never trained on.

## 4 Method

### 4.1 Learning algorithm

We use PPO with the clipped objective [4] and generalised advantage estimation [12], running 2048 simulated cars in parallel [13]. The actor and the critic are separate MLPs with two hidden layers of 512 tanh units. The policy is a Gaussian with a state-independent standard deviation. A cap on that deviation falls log-linearly over training (for example from 0.61 to 0.08), so that the data-collecting policy approaches the deterministic one used at evaluation. Observations are normalised with running statistics. Values are bootstrapped at time-limit truncations [6].

| hyperparameter | value |
| --- | --- |
| parallel cars × rollout steps | 2048 × 64 (131 k samples per iteration) |
| epochs, minibatch | 5, 16 384 |
| learning rate | 3·10⁻⁴ (stage 1), decaying linearly to 0 |
| discount γ, GAE λ, clip ε | 0.997, 0.95, 0.2 |
| entropy bonus | 0.002 (stage 1), 0 afterwards |
| gradient clip (norm) | 0.5 |
| control rate | 25 Hz (40 physics steps per action) |

### 4.2 State distribution

This is the central change. Three mechanisms put the states of a stint into training.

1. **Long episodes.** Episodes may last 600 s (stage 1) and then 1400 s (later stages), several laps up to a full stint. Starts remain at random points and speeds, so every part of the track is visited both from a standing start and at speed.
2. **Worn starts.** With probability 0.5 (then 0.4), a random start uses tyres as a stint leaves them. Wear is drawn uniformly up to 0.35, with each tyre within ±30 % of the common draw. The carcass temperature is drawn from 60–105 °C, and each tread zone lies −10 to +15 K around it. This reaches, from the first iteration, conditions that the policy would otherwise meet only after many clean laps.
3. **Crash replay.** Every car records its state every 0.5 s. When an episode ends in a crash, the state 3 s earlier enters a pool of 4096 states. A quarter of all resets restart from a random state in this pool. This follows GT Sophy's mistake learning [1] and focuses training on the situations the current policy fails in.

### 4.3 Observations

The observation has 148 values, all of which racing simulators report in their telemetry:

- **Car:** velocity, yaw rate, acceleration, wheel speeds, rpm, gear, the applied inputs, the lateral offset and the heading relative to the centreline.
- **Track:** 24 points ahead along the centreline, in the car's frame, with the track width on each side. The first point is 6 m ahead, and each gap is 1.1 times the previous one, so the points reach 530 m. They are dense where precision matters and far enough ahead for braking from top speed.
- **Tyres and car state:** tread temperature and pressure; slip angle, slip ratio and load; wear and carcass temperature; brake disc temperature; body damage.
- **Lap position:** the sine and cosine of the car's share of the lap.

### 4.4 Actions

The actions are steering, throttle and brake; gears change automatically. With a fixed steering range, the same action noise that is harmless in a hairpin is violent at 250 km/h. We therefore scale the steering action with speed. An action of ±1 commands the road-wheel angle that turns the car at 25 m/s², plus 0.07 rad for slip angles and corrections:

$$\delta_{\max}(v) = \min\left(\delta_{\text{lock}},\ \max\left(0.2\,\delta_{\text{lock}},\ \frac{L \cdot 25\ \text{m/s}^2}{v^2} + 0.07\ \text{rad}\right)\right)$$

where $L$ is the wheelbase. The range spans full lock at walking pace and about 5° at top speed.

### 4.5 Reward and termination

Per step, the reward is

$$r = 0.1\,\Delta s \cdot \mathbb{1}[\text{on course}] - 0.02\,n_{\text{off}} - 0.5\,|\Delta\text{steer}| - 0.4\,\textstyle\sum_i g_i - 200\,\textstyle\sum_i \Delta w_i - 50\cdot\mathbb{1}[\text{crash}]$$

where $\Delta s$ is the progress along the centreline in metres and $n_{\text{off}}$ is the number of wheels off the track. $g_i$ is the grip that tyre $i$ has lost to its temperature, pressure, wear and dirt, and $\Delta w_i$ is the tread it wore in the step.

- **Tyre price.** Overheating a tyre costs lap time for minutes, far beyond the horizon of γ = 0.997 at 25 Hz (about 13 s). The grip-loss and wear terms charge that cost immediately.
- **Off-course mask.** Progress made with three or more wheels off the track earns nothing, as in GT Sophy [1]. Section 5.5 shows why this is needed.

An episode ends as a crash on:

- a wall impact faster than 3 m/s;
- 0.5 s with all four wheels off;
- 4 s nearly stationary;
- 1 s facing the wrong way.

### 4.6 Training schedule and model selection

Training runs in three stages on one GPU (AMD RX 9070) and an 8-core CPU, at 30–60 k agent steps per second:

| stage | from | duration | learning rate | action std. (start → end) |
| --- | --- | --- | --- | --- |
| 1 | scratch | 150 min | 3·10⁻⁴ | 0.61 → 0.08 |
| 2 | stage 1 | 85 min | 1.5·10⁻⁴ | 0.08 → 0.05 |
| 3 | stage 2 | 20 min | 1·10⁻⁴ | 0.05 → 0.04 |

Stage 3 enables the off-course mask. The other settings of Sections 4.2–4.5 are the same in all stages. Most of the wall time goes to simulation rather than to network updates. We select checkpoints by the evaluation of Section 5.1, as GT Sophy selected its policies [1]. Stage 3 was stopped after 20 minutes of a 55-minute schedule because that checkpoint was 2 s faster over 10 laps than the last one.

## 5 Experiments

### 5.1 Evaluation protocol

Each policy drives 32 cars for 10 laps from a standing start on the start line, with the lateral position drawn within ±0.3 m. The first car runs the deterministic policy. The others add Gaussian noise with a standard deviation of 0.02 in normalised action units, to test robustness. We report:

- the number of cars that finish, and the crashes per lap driven;
- the steps per lap with three or more wheels off the track;
- the 10-lap time of the noise-free car and the mean over the cars that finish;
- per lap: the hottest tread, the mean grip left by the tyres' condition, and wear.

### 5.2 Main result

| policy | finished | crashes | off course (steps/lap) | 10 laps, first car | mean |
| --- | --- | --- | --- | --- | --- |
| baseline | 0 / 32 | 32 in 73 laps | 3.9 | – | – |
| stage 1 | 32 / 32 | 0 in 320 laps | 0.00 | 1223.4 s | 1228.9 s |
| stage 2 | 32 / 32 | 0 in 320 laps | 0.08 | 1213.5 s | 1218.4 s |
| stage 3 | 32 / 32 | 0 in 320 laps | 0.00 | **1208.2 s** | **1213.1 s** |

The stage-3 car laps in 125.1 s from the standing start, 119.5 s on lap 2 and 120.8 s on lap 10. From lap 3 on the treads stay at 103–105 °C, compared with 137 °C for the baseline. Mean grip falls from 0.982 to 0.957, mostly from wear, which reaches 0.17 after 10 laps. The hot pressure settles about 0.1 bar above the tyre's optimum, which costs about 2 % grip; this is a matter of car setup, not of driving.

### 5.3 Ablations

We trained two variants of stage 1 to separate the contributions.

- **Weak tyre price:** grip-loss weight 0.1 and wear weight 40.
- **Old state distribution:** 180 s episodes, fresh tyres and no crash replay, with every other change of Section 4 kept.

Each variant was evaluated with 16 cars.

| variant | iterations | finished | lap-10 grip | hottest tread |
| --- | --- | --- | --- | --- |
| stage 1 as described | 404 | 13 / 16 | 0.940 | 135 °C |
| stage 1 as described | 2044 | 16 / 16 | 0.964 | 111 °C |
| weak tyre price | 1163 | 10 / 16 | 0.818 | 196 °C |
| old state distribution | 408 | 0 / 16 | – | – |
| old state distribution | 1336 | 0 / 16 | – | – |

The old state distribution is decisive. With it, the policy lapped faster on its best lap (114.4 s, partly by cutting a chicane, see Section 5.5). Yet every car crashed on lap 3, more than 240 s after the start and well beyond the 180 s training episodes. The tyre price mainly affects pace over the stint. Without it, the policy slid its front tyres to 196 °C and was about 6 s per lap slower by lap 8.

### 5.4 Driving technique in a hairpin

In a slow hairpin, the minimum speed is almost the same for both policies (61.5 km/h for stage 3, 61.0 km/h for the baseline), and so is the exit speed. The difference is in technique and consistency.

- **Baseline:** kept the throttle part open while braking for 1.9 s, swung wide before turning in, and crashed on later laps.
- **Stage 3:** brakes in a straight line with a single step of pedal overlap. It releases the brake as it turns in (trail braking), clips the inside at a late apex, and reaches full throttle within 15 m of the apex while using the full width on exit ("slow in, fast out").
- **Consistency:** stage 3's braking point varies by 3 m between lap 2 and lap 10.

### 5.5 Reward hacking: cutting a chicane

A policy fine-tuned from the old-state-distribution run learned to drive straight across a chicane at about 200 km/h, spending 20 steps per lap (0.8 s) with all four wheels off. Its 10-lap time of 1183.3 s looked 25 s better than the stage-3 result, but the laps are invalid. The cause was the reward: progress along the centreline paid more than the small per-wheel off-track penalty cost. Masking progress with three or more wheels off, as in GT Sophy [1], removed the gain. Continued training from the cutting policy with the mask did not relearn braking for the chicane within 55 minutes, and all of its cars crashed. A reward should be checked against every shortcut it allows [11]. Our evaluation now reports off-course steps to catch this.

## 6 Discussion and limitations

- **What matters most.** The main lesson is that what a policy trains on matters more than how. Algorithm settings did not cause the long-run failures. They came from states that the training never produced. Worn starts and crash replay are cheap ways to produce such states, and long episodes let them arise naturally.
- **Single seeds.** Each configuration was trained once, so the differences are indicative rather than precise.
- **Steering saturates.** The speed-scaled steering action reaches its limit at the apex of the slowest hairpin, so the range could be wider there.
- **One car and one track.** We trained on one car and one track. The method has no track-specific parts, but we have not tested how it transfers.
- **Tyre model.** The tyre model is simplified: it has no graining or blistering, and its wear curve is linear.
- **Future work.** A policy that adapts from a history of observations [14], rather than from direct telemetry of its tyres, could transfer to simulators that report less.

## 7 Conclusion

Racing policies trained on short episodes and fresh cars fail over a stint because a stint takes them into states they never trained on. Training on those states by using long episodes, worn and hot tyre starts, and restarts before earlier crashes produced a policy that drove 320 laps without a crash or a track-limit violation. Pricing tyre degradation in the reward kept the tyres in their working range. The `longrun` evaluation in open-racing reproduces these measurements for any policy.

## References

1. P. R. Wurman et al. Outracing champion Gran Turismo drivers with deep reinforcement learning. *Nature* 602, 223–228, 2022.
2. F. Fuchs, Y. Song, E. Kaufmann, D. Scaramuzza, P. Dürr. Super-human performance in Gran Turismo Sport using deep reinforcement learning. *IEEE Robotics and Automation Letters* 6(3), 4257–4264, 2021.
3. Y. Song, A. Romero, M. Müller, V. Koltun, D. Scaramuzza. Reaching the limit in autonomous racing: Optimal control versus reinforcement learning. *Science Robotics* 8(82), 2023.
4. J. Schulman, F. Wolski, P. Dhariwal, A. Radford, O. Klimov. Proximal policy optimization algorithms. arXiv:1707.06347, 2017.
5. S. Ross, G. Gordon, D. Bagnell. A reduction of imitation learning and structured prediction to no-regret online learning. *AISTATS*, 2011.
6. F. Pardo, A. Tavakoli, V. Levdik, P. Kormushev. Time limits in reinforcement learning. *ICML*, 2018.
7. T. Salimans, R. Chen. Learning Montezuma's Revenge from a single demonstration. arXiv:1812.03381, 2018.
8. C. Florensa, D. Held, M. Wulfmeier, M. Zhang, P. Abbeel. Reverse curriculum generation for reinforcement learning. *CoRL*, 2017.
9. A. Ecoffet, J. Huizinga, J. Lehman, K. O. Stanley, J. Clune. First return, then explore. *Nature* 590, 580–586, 2021.
10. J. Tobin, R. Fong, A. Ray, J. Schneider, W. Zaremba, P. Abbeel. Domain randomization for transferring deep neural networks from simulation to the real world. *IROS*, 2017.
11. D. Amodei, C. Olah, J. Steinhardt, P. Christiano, J. Schulman, D. Mané. Concrete problems in AI safety. arXiv:1606.06565, 2016.
12. J. Schulman, P. Moritz, S. Levine, M. Jordan, P. Abbeel. High-dimensional continuous control using generalized advantage estimation. *ICLR*, 2016.
13. N. Rudin, D. Hoeller, P. Reist, M. Hutter. Learning to walk in minutes using massively parallel deep reinforcement learning. *CoRL*, 2022.
14. A. Kumar, Z. Fu, D. Pathak, J. Malik. RMA: Rapid motor adaptation for legged robots. *RSS*, 2021.
